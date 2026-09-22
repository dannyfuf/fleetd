# Board workflows, phase 9: app and docs — keys, palette, confirm, end-to-end scenario — Tracker
> Plan: ./board-workflows-2026-09-20-phase-9-plan.md
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
  (Not tickable by any phase-9 owner, for phase 5's reason: the tree was never clean — the nine
  phases were built in parallel in one worktree — and `make lint`/`make test` belong to the
  integration stage rather than to a task.)
- [x] I am ready to start.

## Tasks
- [x] P9-T01 — Five actions, their keys and their handlers
      verified: `cargo test -p fleet-app --lib screens::board` — 26 green, seven of them new in
      `screens/board/runs.rs`, the new home of the three run keys (`attach_run`, `cancel_run`,
      `run_now`); `b`/`m` are `actions::pick_blocked_by`/`pick_agent`, two `open_picker`
      wrappers, so the read-only refusal is the pickers' own. Every refusal is a toast in the
      contract's words — `{KEY} has no run`, `{KEY} has no live run`, `{column name} has no
      action`, `actions::needs_named` for all three — and `RunTarget::to_attach`/`to_cancel`/
      `to_run` are pure, so the sentence a key says is what a test reads. Hint lines: the run
      row's `A attach · X cancel · > re-run` (`detail::run_hints`, pinned by
      `the_run_row_names_the_three_keys_that_act_on_it`) and the dialog footer's `A attach ·
      X cancel · > run` before `esc close`. The keymap rows, the actions and `docs/KEYMAP.md`
      were already in from the skeleton and were not touched.
- [x] P9-T02 — Attach, and what the attached thread says
      verified: `cargo test -p fleet-app agent_thread` — 113 green, one new. In: `A` resolves
      the focused card's live run, else its newest, and attaches `CardRun.thread_id` through
      the existing `screens::workspace::reopen_agent_tab` (the path `^s u` and a delegation row
      already use — no new request, no copy of `open_thread`); from the detail the dialog
      closes first. A run that never reached a thread answers `{KEY} has no run`, the same
      sentence as a card that never ran, because there is nothing to open either way.
      Wired at integration, 2026-09-21: `AgentThreadView.card_caller` +
      `sync_card_caller` (`screens/agent_thread/{mod,sync}.rs`) put
      `presentation::card_metadata_segment` first in `refresh_metadata` and
      `CARD_COMPOSER_PLACEHOLDER` ahead of the child arm in `sync_composer`;
      `screens/workspace/agent.rs::card_caller_context` is the join that feeds them, read beside
      `caller_context`. The segment is targeted again under `presentation::CARD_TARGET_PREFIX`
      (`card:`), parsed in `agent_thread/view.rs::on_target` into a new
      `AgentThreadEvent::SelectCard`; both that event and `^s u` land in one
      `screens/workspace/actions.rs::jump_to_card`, which stages
      `BoardState::pending_focus` (applied by `apply_board_view`) and then opens the board tab,
      so a selection survives the frames between asking for the tab and the view arriving.
      Both `#[cfg_attr(not(test), expect(dead_code, …))]` attributes in `presentation.rs` are
      gone, and `CardCaller` gained the `card` field the target names.
      verified (integration): `cargo test -p fleet-app` and
      `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean.
- [x] P9-T03 — Cancel with a confirm, `MoveCancelsRun`, and the sticky error
      verified: `cargo test -p fleet-app --lib screens::board confirm` — 26 + 27 green, six new
      tests. In: `move_cancels_run` (`screens/board/actions.rs`) raising the confirm only for a
      live *card-called* child behind `latest_run.id`, `stage_move_target` + the `commit` arm
      sending `MoveCard { cancel_run: true }` through `send_card_reporting`/`Refusal::Sticky`,
      and `ask_cancel_run` + `stage_card_run_cancel` + `commit_card_run_cancel` for `X`.
      Already done elsewhere and **not** rebuilt: the failed-start sticky error (phase 6,
      `AppState::report_failed_start`, pinned by
      `a_run_that_never_reached_a_thread_raises_the_sticky_slot_once`), and `X`'s target
      resolution and `{KEY} has no live run` refusal (P9-T02's `screens/board/runs.rs`).
      P9-T02's owner wired `ask_cancel_run` into `runs::cancel_run` the same afternoon, so no
      `CONTRACT STUB` is left in `fleet-app` and `cargo clippy -p fleet-app --all-targets
      --all-features -- -D warnings` is clean.
- [x] P9-T04 — Six palette commands
      verified: `cargo test -p fleet-app palette` — six variants, labels, icons, action names,
      `ALL`, `valid_with` per contracts §5.5 and `run_command` dispatching to the P9-T01
      handlers, plus two tests (the run rows over a worktree board only; the other three follow
      the board they edit).
- [x] P9-T05 — `scenarios/board/workflow-chain.scenario`, end to end
      verified: `make harness-one SCENARIO=scenarios/board/workflow-chain.scenario` in both
      lanes — `LANE=virtual` falls back to headless on this machine (no compositor, phase 6's
      note and §11) and runs the identical line set, because the file takes no `shot` and is
      therefore in the headless subset by construction (§7) — and, the run that matters most,
      `cargo test -p fleet-app --test harness_headless`, where it passes **inside** the loaded
      corpus (`ran board/workflow-chain.scenario in 12.7s`, 88 lines). Plus `cargo test -p
      fleet-app` and `cargo clippy -p fleet-app --all-targets --all-features -- -D warnings`.
      The scenario drives, against real children: `A`'s refusal and then the attach (the run's
      own tab, `↳ AGE-1 — …`, composer focused), `^s u` back to the board tab with the card
      focused, `^s d` listing the run as a *top-level* AGENTS row (`badges[1] == caller`)
      labelled by its tab title, `[` accepted with `y` on AGE-1 and `]` refused with `n` on
      AGE-2 — both `MoveCancelsRun` sentences read back word for word — `X` with its confirm
      and the run going terminal (`card.runs` row 0 reads `cancelled …`), and `>` twice: the
      `{column} has no action` refusal and the re-run ask. Three things it needed that did not
      exist are under **Notes** below: the additive `dialog.message`, the `show_board_tab` fix,
      and the deviation on where the card chain comes from.
- [x] P9-T06 — The four documents and the cross-document audit
      verified: audit half, 2026-09-21 — `make lint` clean and `cargo test -p fleet-app keymap`
      33 passed, including `board_documentation_and_bindings_match_in_both_directions`, which is
      what proves `KEYMAP.md` row for row against `keymap::table()`. Each authoritative claim was
      re-grepped against the shipped code rather than re-read: the four model constants
      (`MAX_RUNS_PER_CARD` 20, `MAX_REPORT_COMMENTS_PER_CARD` 3, `REPORT_EXCERPT_CAP_BYTES` 8
      KiB, `PENDING_AMBER_AFTER_SECS` 60), every `BoardError::Invalid` sentence of §11.4, every
      activity sentence of §11.3, the preset's seven columns, the `board-workflow` fixture (four
      cards in Todo, 1→3 and 1+2→4, `max_live_runs` 1) and `agent::GATE_BUDGET` = 20 s, the
      refusals and hint lines of §11.8–§11.10, the nine-field `fleet subagent list` line, the
      slot-007 rebuild and the `Recorded` delivery, `document_version` and the store's `1..=2`.
      `TESTING-HARNESS.md` §2 is byte-identical to `main` but for the one agreed token
      (`|board-workflow` in the fixture enumeration) — checked by extracting §2 from both and
      diffing — and no pre-existing target name in §3 changed; §3/§4/§5's other changes are
      additions plus the two non-additive projection changes the document itself names.
  - The **writing half is done** (i:app-docs): `KEYMAP.md` — the fourteen §5.5 rows were already
    landed by the skeleton and a:settings and match `keymap::table()` row for row (`cargo test -p
    fleet-app --lib keymap` 33 passed, incl.
    `board_documentation_and_bindings_match_in_both_directions` and
    `the_run_keys_are_absent_from_the_hubs_board`); what was missing and is now written is the
    prose under the board table: the run keys' two contexts, and `b` shadowing the Hub's
    open-in-browser. `APP-CONTRACTS.md` — the one-way sentence ("the app never starts a run; it
    asks the daemon to change a card and draws what comes back", one `MoveCard { cancel_run }`
    rather than two requests, plus the `refresh_card_marks` call sites and the `marks.revision`
    projection key) in the Board extension-points section, and the `b`-shadow bullet in the
    arbitration list beside the `q`/`f` ones. `NATIVE-AGENTS.md` §15.3 — a card is a caller like
    a thread, `depth = 1`, no per-caller ceiling, the daemon-wide 8 and the board's own
    `max_live_runs`, then the attach / cancel / steering rules; §2's "Where state shows" gains
    the sentence that the board reuses that vocabulary unchanged. `UX-SPEC.md` Board "Keyboard"
    gains the six-key table with each refusal sentence and the two confirms, quoted from
    `screens/board/runs.rs`, `screens/board/actions.rs::card_run_cancel_draft` and
    `dialogs/confirm{.rs,/policy.rs}`.
  - **The audit half closed it, 2026-09-21**, and found six disagreements, all of them in the
    documents rather than in the code — which is the finding worth recording: nine phases of
    parallel writing left the prose accurate about what each phase built and stale only where a
    *different* phase's change had crossed it.
    1. `BOARD.md` §11.7's outcome table — the non-terminal row (`Starting`/`Running`/`Blocked`/
       `Settling`) sat *below* the paragraph that follows the table, so it rendered as a stray
       line and not as a row. Moved back into the table, and the paragraph now says "every
       terminal row but the first", which is what it always meant. The behaviour it states is
       `Boards::on_run_delivered`'s own: a non-terminal delivery answers `Ok`, which closes the
       delivery row and leaves the run row open.
    2. `.claude/skills/fleet-subagent-cli/references/commands.md` still said `fleet subagent list`
       prints **eight** fields and omitted `caller`; `human::subagents_with_keys` prints nine,
       and `NATIVE-AGENTS.md` §15.5 already said so. Fixed, with what `<caller>` reads.
    3. The same file's Delivery table had no `recorded` row, and `SKILL.md`'s field/word summary
       repeated both errors. Both fixed.
    4. `.claude/skills/fleet-board-planning/references/checklist.md` had no items for the new
       "Building a chain" section, whose whole point is the order that costs a debugging hour
       when broken. Added, plus one review finding.
    5. `docs/README.md` assigns each document a domain, and `BOARD.md`'s row did not mention the
       automation engine at all. Named it.
    6. `UX-SPEC.md` § Board introduced its six-key table as keys that "act on the focused card's
       *run*… bound on both board surfaces and inside the card detail". Two thirds of that is
       wrong by `keymap::table()`: `C` acts on the **board**, is bound on both boards and is
       **not** in the card detail (`keymap::tests` asserts the absence at :1343), and `A`/`X`/`>`
       are not on the Hub's board, which the paragraph below the table already said. The preamble
       now says which key is bound where; the table and the paragraph were already right, which is
       why six readings of the table never caught the sentence above it.
    Also `TODO.md` §11 (remote-host delegations), which now names the sibling refusal a **card**
    caller gets — `automation is unavailable on a worktree owned by host <id>` — because the two
    are one missing piece and lifting either should lift the other.
    `docs/research/agents-contracts.md` needed nothing: its `DelegationCaller`, `Recorded` and
    slot-007 paragraphs match `delegation.rs`, `migrations.rs` and `delegations.rs` as shipped.

## Notes / decisions log
- 2026-09-21 (P9-T06, audit half) — **most of what this task was scoped to write had already been
  written.** The task text was drafted against a tree where only phase 1 had landed; by the time
  it ran, every phase's doc half was in the working tree, so the audit's job was to *check* rather
  than to author, and the five findings above are all it produced. The rule that made the
  difference: for each claim a document makes, grep the shipped symbol or the shipped string
  before rewriting a sentence. Two of the five were caught by a mechanical check rather than by
  reading — a markdown-table integrity pass over every touched document found the orphaned row in
  `BOARD.md` §11.7 that six careful readings had not — and the other three by comparing two
  documents that state the same fact (`NATIVE-AGENTS.md` §15.5 against the subagent skill).
  The trackers' Kickoff blocks were reconciled at the same time: phases 6–9 had left all three
  boxes unticked, which read as "never started" for work that is done and verified. They now carry
  phase 5's honest form — read and ready ticked, the clean-tree baseline left open with the reason
  it was never tickable by anyone building in a nine-agent shared worktree.
- 2026-09-21 (review fixes, P2) — three app fixes from the three-reviewer pass:
  `screens/board/runs.rs` now tells `A`'s two refusals apart (`{KEY} has no run` for a card that
  never ran, `{KEY}'s runs never reached a thread` when it ran but no run of it did) in the
  CLI's own words; `views/board_card_detail.rs` imports the transcript's
  `DELEGATION_RESULT_COLLAPSE_LINES` instead of carrying a second literal, which is what
  contracts §5.3 asked for; and `dialogs/card_picker/schema.rs`'s `PreparedKey` gains
  `vocabulary`, because the Model and Effort rows are read from a *projection* and
  `summaries_revision` does not move when a harness declares its models — a picker opened before
  that landed cached an empty catalogue. `state::agents` gained `vocabulary_revision`, bumped by
  `SessionConfigured` and by installing a snapshot. Also: `state/board.rs`'s `clamp_board_focus`
  doc paragraph had slid onto `select_card`; it is back where it belongs.
- 2026-09-21 (scenario:corpus) — **the corpus was run a second time with
  `--continue-on-failure`, because a failed `shot` ends its scenario and everything after it goes
  unchecked on this machine.** 66 of the 71 files then had no failing line that was not a `shot`;
  of the 5 that did, `agents/scroll-wheel` and `board/workflow-detail` were green alone,
  `agents/subagent-reopen-closed-caller` is the known 1-in-3 flake already logged below,
  `hub/jobs-panel` lines 28 and 40 are a real wrong assertion in a part of the corpus nothing
  here had ever executed (under **Follow-ups**, owner: `scenarios/hub/`), and
  `board/workflow-columns-context` line 39 was this round's own §9.3 race —
  `assert lists.board.rows[0].label` the instant `hub_tab == Board` held, while the view with
  the columns in it was still a few frames away. That one is fixed here, as an `await` on the
  first column; 3 runs of 3 green afterwards.
- 2026-09-21 (scenario:corpus) — **the whole corpus was run, and `make harness` cannot pass on
  this machine.** There is no compositor here, so the virtual lane falls back to headless and
  every scenario carrying a `shot` fails on that line — 38 of the 71 — and the suite stops at the
  first one. §11 already records the lock-screen half of this gap; the no-compositor fallback is
  the same gap with no session at all. The corpus was therefore run **scenario by scenario**
  (each file, `--lane virtual`, falling back) so one failure could not hide the rest, and every
  failure whose failing line was not a `shot` was then re-run alone. Result: 5 such failures,
  3 of them load flakes that are green alone (`board/navigation`, `board/card-create-editing`,
  `agents/subagent-attach-from-picker`) and 2 real, both pre-existing and both invisible until
  now because each file is pixel-bound and so sits out `harness_headless`, the only corpus run
  this machine can finish. Both are fixed below. `make harness-headless` is green.
- 2026-09-21 (scenario:corpus) — **`lists["board.cards"]` was not the column the pane draws, and
  `workspace/board-tab.scenario` is where that showed.** The projection built the row list
  straight off `view.cards`, which is document order — a card keeps the slot it was created in
  and only `position` says where its column shows it — while the pane and
  `AppState::select_card` both order by position through `views::board_screen::visible_cards`.
  So after `]` the scenario read `rows[1]` at the index `focused` had just given it and got the
  other card, and a filtered board reported rows it was not showing. Fixed at the root: both the
  card rows and the `board` row badges now come from `visible_cards`, so the two vocabularies
  cannot name different cards and the list honours the filter it already reported. Pinned by
  `state::harness::tests::workflow::the_card_list_is_the_column_in_the_order_the_pane_draws_it`;
  `docs/TESTING-HARNESS.md` §3 says so in the same change. The same scenario also asserted
  `mode == Native` 11 ms after an `await` on a different field, with the select mutation still
  settling (the report says `settling_mutations = 1`); that line is now an `await`, which is
  what §9.3 asks for and what `94f45d7` did to the other race-prone scenarios.
- 2026-09-21 (scenario:corpus) — **`click agents.composer` typed into nothing, and the target was
  the bug.** `agent-approval.scenario` clicked the composer, typed and pressed `⏎`, and the
  thread stayed idle for ever: no turn, no toast, and `focused` still reading `agents.composer`.
  The name was on the whole composer stack — editor row, host badge, metadata strip — and a
  `click` resolves to the centre of its rectangle, which lands in the strip; the mouse down
  reached no editor and every keystroke after it was dropped in silence. Clicking an explicit
  point inside the editor row passed, which is what named the cause. Fixed by moving
  `.harness_target("agents.composer")` onto the editor's own row in
  `screens/agent_thread/view.rs`. Pinned by
  `fleet-ui-kit … multiline_input::tests::a_click_into_the_composer_leaves_it_typing` (the editor
  itself was never at fault: click, type and submit all work on it) and by the scenario, whose
  comment now says what the click is there to prove. `docs/TESTING-HARNESS.md` §3's target notes
  carry the rule. Not the fix: a focus redirect on the tab's own handle, tried and reverted —
  gpui's `is_focus_in` is false for a move from a descendant to its ancestor, so it never fires,
  and focus was never the thing that moved anyway.
- 2026-09-21 (P9-T05) — **the scenario found a real app bug, and it is fixed here.** `^s u` from
  a card run's thread reached `jump_to_card`, which claimed the scope, sent `SelectTerminal` for
  the board tab — and left the workspace showing the agent composer, for thirty seconds, with no
  sentence anywhere. Root cause: a worktree's *active agent thread* outranks its selected
  terminal in everything that draws the workspace (`tab_rows`, `active_tab_is_fleet_drawn`), and
  `show_board_tab` never left it. The strip's own selection has always left the tab first
  (`agent::select_tab` → `leave_agent_tab`); the two keys that jump to the board did not, so
  `ctrl-s b` from inside an agent tab was broken in exactly the same way — `docs/KEYMAP.md` has
  promised both behaviours since before this phase. Fix: one `leave_agent_tab` in
  `show_board_tab`, which is the path both keys share. Regression test:
  `screens::workspace::tests::showing_the_board_tab_leaves_the_agent_tab_in_front_of_it`.
- 2026-09-21 (review fix F8) — **two harness changes are contract changes to §3, not additions,
  and the doc now says so.** Contracts §5.6 lists this phase's harness work as additive; two of
  the entries below are not. `board.cards` and the `board` badge changed *population* — filter
  applied, archived excluded, position order — and `dialog.message` went from "always null" to
  carrying a sentence. Both are the right behaviour and both stay; what was wrong was the label,
  so `docs/TESTING-HARNESS.md` §3 now names them as meaning changes a scenario written before
  them can read differently. The §8 `dialog.message` bullet was also re-wrapped: its continuation
  lines had drifted to column 0 inside a `-` item and past the file's width.
- 2026-09-21 (P9-T05) — **`dialog.message` now carries the Confirm dialog's consequence, and
  that is the additive field this task needed.** The plan asks the scenario to read the move
  confirm's sentence back; §11 said `message` was `null` everywhere, so there was no oracle for
  the one line §3.8.3 has the user accept. `message` is already in the frozen `DialogSnapshot`
  (§3), so filling it is additive within version 1: `dialogs::dialog_message` crosses the draft
  in on the same seam `dialog_fields` uses, and `confirm::consequence` chooses the sentence in
  the order `confirm::render` chooses its card — delegation cancel, then card-run cancel, then
  `ConfirmRequest::consequence` over `view::facts_for`, which was split out of `facts_card` so
  the drawn sentence and the reported one cannot drift. Every other dialog still answers `null`:
  its body is elements, not a sentence. Pinned by
  `dialogs::confirm::tests::the_reported_consequence_is_the_sentence_the_open_card_draws`;
  `docs/TESTING-HARNESS.md` §3 and §11 say so in the same commit.
- 2026-09-21 (P9-T05) — **the three-card chain comes from the `board-workflow` preset, not from
  a scripted thread's `shell` steps.** The plan predates P6-T05's preset, which already seeds
  AGE-1 blocking AGE-3 and AGE-1 + AGE-2 blocking AGE-4 through the seeding daemon's own typed
  operations; §9.2 forbids building with scenario lines what a preset seeds, and driving `fleet
  board card new --blocked-by` through a transcript would have needed a new transcript and a new
  preset for state the corpus already has. The scenario reads the links off the face and drives
  only the moves. What is therefore *not* covered here is the `fleet board` CLI surface itself —
  it is phase 2's, and `make smoke-workflow` is where it is exercised.
- 2026-09-21 (P9-T05) — **what a scenario has to wait for before a move confirm, in two wrong
  answers and one right one.** Contracts §5.5 asks the confirm only while a live card-called
  child stands behind the card's newest run, and the five-second `GATE_BUDGET` is how long one
  ever stands. (1) Awaiting the `working` mark is not that condition — the mark is also drawn
  from the board view's own `live_runs` join, which moves before any child exists — and the
  first draft got the daemon's `AGE-1 is working; pass --cancel-run to move it` in the sticky
  slot instead of a dialog. (2) Awaiting `agents.delegations[0]` *is* the condition, but only
  for the first run this board ever starts: under `cargo test`'s load that run was already
  `cancelled` by the time the await began, and index 0 never becomes live again — the restart
  writes index 1, 2, 3… That draft passed alone and failed inside `harness_headless`, which is
  the only run that proves a scenario. (3) What the file does now is await the tile's `needs
  you`: it means *this* run's child has reached its gate, whatever run that is, so the keystroke
  starts at the beginning of the widest window there is, and a loaded machine that misses one
  cycle gets the next five seconds later. The two confirms are in two windows — `[` on AGE-1,
  `]` on AGE-2 — rather than back to back, so neither has to fit in the other's remainder.
- 2026-09-21 (P9-T05) — **a second load flake in the corpus, not this phase's and not fixed
  here:** `agents/subagent-reopen-closed-caller.scenario` line 13, `assert agents.threads[0]
  .attached == false && mode == Terminal`, reads `mode` as `Native` about once in three runs
  (measured 1 of 3 alone, and once inside `harness_headless`). It is the shape §9.3 warns
  about: the line `assert`s a field that is still settling after an `await` on a *different*
  one — the closed tab leaves the strip before the workspace's terminal mode is resynced. The
  one-line fix is to make it `await mode == Terminal 10000` on its own line, and it belongs to
  `corpus-agents` rather than to this task. **Owner: whoever owns `scenarios/agents/`.**
- 2026-09-21 (i:app-docs) — `NATIVE-AGENTS.md` §15.3 deliberately does **not** describe the card
  run's tab chrome. `screens/agent_thread/presentation.rs::{card_metadata_segment,
  CARD_RUN_COMPOSER_PLACEHOLDER}` are written and tested but still carry
  `#[cfg_attr(not(test), expect(dead_code, …))]` — P9-T02's remaining wiring, whose exact edits
  are under **Follow-ups** here. A doc is authoritative, so it states what attach actually does
  today (an ordinary agent tab) and nothing about a `for FLT-12 · Ready · Fleet` segment no tab
  draws yet. Whoever lands that wiring adds the sentence in the same commit.
- 2026-09-21 (i:app-docs) — The `b`-shadow arbitration went in `APP-CONTRACTS.md` §3's
  "Arbitrations against `KEYMAP.md`" list, beside the `q`/`f` entries, rather than in §6: §6 is
  about action dispatch and the help overlay, and this is a context-precedence fact about two
  bindings of one key. §6 needed no change.
- 2026-09-21 (contracts:app skeleton) — The app skeleton of phases 6 to 9 landed first so the
  eight app implementers can start at once. From this phase: the six `board` actions, the §5.5
  keymap rows with their `docs/KEYMAP.md` rows, one handler per action, `ConfirmRequest::
  MoveCancelsRun` with its real `title`/`consequence`/`icon`/`action_label`/`target` arms (the
  `commit` arm is a stub), and P9-T04 in full.
- 2026-09-21 (P9-T03) — The column a confirmed `MoveCancelsRun` moves into is **staged beside**
  the request, not carried in it: contracts §5.5 fixes the variant's four fields, and a status
  id is not a sentence. `[` / `]` put it in the same per-window staging set the delegation
  cancel uses (`PendingBoardConfirms`), the dialog adopts it in `seed`, and a staged column that
  did not survive a reconnect makes `y` do exactly what `n` does — never a move into a column
  the dialog can no longer name.
- 2026-09-21 (P9-T03) — `X` is split the way the two files are: `runs::cancel_run` owns *which
  card and may it* (it already resolves the target and refuses in the daemon's words), and
  `actions::ask_cancel_run` owns *what exactly stops, and did the user say yes*. The confirm's
  one risk line is built where the card is, because a live child and an owed slot are different
  facts, and only an accepted dialog reaches the wire — through `send_card_reporting`, so a run
  that ended between the key and the `y` is refused into the sticky slot instead of silently.
- 2026-09-21 (P9-T01/T02) — The three run keys live in a new `screens/board/runs.rs`, not in
  `shell/root/board.rs`: every other board key's handler is one delegating line, and the shell
  is a dispatch file. `RunTarget` is read once from the mirror and answers each key with either
  an id or the sentence that refuses it, so the refusals are unit-tested without a window,
  which is how every other board decision here is tested.
- 2026-09-21 (P9-T02) — `A` reuses `reopen_agent_tab` rather than sending `AgentThreadOpen`
  itself: the plan named `requests.rs`'s open path, but that request is what a *mounted* tab
  makes for its transcript window, and the tab itself is created by attaching and selecting the
  thread — which is exactly what a finished delegated child does today. Nothing new was added
  to `screens/workspace/agent/requests.rs`.
- 2026-09-21 (P9-T02) — The card caller's metadata segment is drawn **untargeted** until `^s u`
  can act on it. A targeted segment renders in the accent tone and invites a click; one whose
  jump is not wired yet would be an affordance that does nothing, which §5.5's own reason for
  keeping `A`/`X`/`>` off the Hub's board forbids.
- 2026-09-21 — `Board: Blocked by` and `Board: Agent` are listed on `has_card`, not on the
  plan's bare `on_board`: they act on the focused card exactly as `Board: Labels picker` does,
  and §3.9 lists no row that can only refuse. `Board: Columns` follows `on_board`, like
  `Board: Settings` beside it.
- 2026-09-21 (integration) — the card segment is targeted after all. a:keys left it untargeted
  because `^s u` could not act on it; now that it can, an untargeted segment would be the
  affordance §5.5 forbids in the other direction — the one visible statement of where this run
  belongs, that does not take you there. The target is prefixed (`card:<id>`) so one `on_target`
  can serve both a bare thread id and a card without either being mistaken for the other.
- 2026-09-21 (integration) — the jump stages `BoardState::pending_focus` before it asks for the
  board tab, and `apply_board_view` applies it. A bare `focus_card` after `open_board_tab` does
  nothing when the tab is being created: the mirror has no view yet, and the selection would be
  dropped on the floor exactly in the case the key is most useful. The staged card is cleared by
  the first view that holds it, so a stale stage cannot steal a later selection.
- 2026-09-21 (integration) — `sync_composer`'s card arm sits *before* the child arm. A card run's
  child usually has no caller thread at all, so the order is only observable for a thread that
  somehow has both, and there the card is the more specific fact about where the reply goes.

## Follow-ups
- **No phase of this initiative ever ran a real provider, and a manual smoke is owed before the
  feature is announced.** Every run any test, scenario or `make smoke-workflow` has ever started
  was a `fleet-harness agent` scripted binary replaying a JSON transcript: deterministic, and
  deliberately so, but it never spawns `claude` or `codex`, never pays a harness probe, never
  negotiates a real permission mode, and never reports a real token count or cost. Nine phases of
  green therefore say the *board* is right and say nothing about the provider seam under it. What
  is owed, on a real worktree board with both binaries on `PATH` and against the user's own
  daemon: apply the workflow preset; build a three-card chain with `--blocked-by`; move the head
  into In Progress and watch a real child start; `A` to attach its tab and read the transcript;
  `X` on a second card and confirm the cancel; `>` to re-run a card that ended; let one run
  succeed and confirm `on_success` moves the card and the report comment carries the
  files-changed section with real paths; repeat for the other provider, including one
  `skill:deep-review` column, which is Claude-only by validation. The parts most likely to be
  wrong are the ones no transcript exercises: the harness probe's timeout under
  `AGENT_HARNESS_TIMEOUT`, the resolved `mode` a real CLI accepts, and whether a real child's
  report text survives `REPORT_EXCERPT_CAP_BYTES` the way the excerpt tests assume.
  **Owner: whoever announces the feature. Do not announce it before this has been done once.**
- **FIXED 2026-09-21 — a card-called child that goes `blocked` does not repaint the board tile,
  and the board face can lag a run by the whole of that run.** Root cause: `run_mark` read a
  card's run only from `card.runs`, and the app learns of a run exclusively through a full
  `EnsureWorktreeBoard` reload claimed off `BoardChanged`. Until that round trip landed the card
  held no run at all, so the `refresh_card_marks` call contracts §5.2 puts after a card-called
  `DelegationChanged` had nothing to fold and the tile stayed blank — then painted whatever the
  reload happened to catch, which for a five-second child is usually the terminal state
  (`Cancelled` → no mark, `Succeeded` under an auto-advancing column → no mark). The client was
  never the problem: `fleet-client`'s `hello_client` has always named `board.automation`
  (`crates/fleet-client/src/connection.rs:914-924`, pinned by
  `hello_names_the_board_automation_capability` at `:1328`), so the daemon's per-peer filter
  (`server/connection/events.rs:49-52`) was letting every card-called `DelegationChanged` through
  and the app's mirror held them all along.
  The fix folds the mirror in by caller: `AgentsState::live_card_runs(board)` keys the live
  card-called delegations by card, `AppState::run_mark` reads that before `card.runs`, and a run
  the card's own row has already given an outcome outranks a mirror row that has not caught up
  (`crates/fleet-app/src/state/{agents.rs,board.rs}`; `docs/BOARD.md` §11.8 and
  `docs/APP-CONTRACTS.md` moved with it). Measured after the fix, headless lane:
  `board/workflow-detail` line 82 is 6/6 green at ≤20 ms where it had been 0/3; `workflow-chain`
  line 113 and `workflow-marks` line 108 resolve in ≤30 ms; and seven probe runs sampled 250 ms
  after the move all read `needs you` with the delegation already `blocked`, which is symptom (1)
  gone. Regression tests:
  `state::board::tests::{a_card_called_child_paints_working_before_the_board_records_its_run,
  a_card_called_child_that_starts_blocked_paints_needs_you_before_the_board_records_it,
  a_run_the_card_has_already_finished_outranks_a_mirror_row_that_has_not_caught_up}`.
- **FIXED 2026-09-21 — `[` on a card the tile already calls `working` sent a plain move and
  collected the daemon's `Conflict` (`workflow-chain:116`, deterministic under load).** The
  previous fix (the entry above) made the face mirror-first, so `needs you` reaches the tile the
  moment the child opens its gate — a whole `EnsureWorktreeBoard` before the card carries the
  run. `workflow-chain:109` therefore passes in that window, and the very next `[` reached
  `screens/board/actions.rs::move_cancels_run`, which gated on `card.runs.last().is_live()` and
  then looked the delegation up by `latest_run.id`. With no recorded run both halves said no, the
  plain `MoveCard` went out, and the daemon answered `conflict: AGE-1 is working; pass
  --cancel-run to move it` into the sticky slot with no dialog ever opening
  (`/tmp/fleet-harness/20260922-001522-workflow-chain/dumps/failure-116.json`: mirror holds
  `d1a19571… blocked`, caller `wt-acme-api-agent/26531268…`, tile `needs you`).
  The fix gives the keys the same join the face uses: `AgentsState::live_card_run(board, card)`
  (`state/agents.rs`) answers per card off the delegation's own `caller == Card { board, card }`,
  `actions::live_card_child` filters it with the card's own terminal outcome exactly as the mark
  fold does, and `actions::has_live_or_pending_run` — now consumed by `runs.rs::target` too — is
  what both `[`/`]` and `X` read. `move_cancels_run` no longer looks at `card.runs` at all.
  Contracts §5.5, `docs/BOARD.md` §11.8 and `docs/UX-SPEC.md` § Board moved with it.
  Measured: `board/workflow-chain --lane headless` 5/5 green, line 116 resolving in 2–3 ms where
  it had timed out at 15 000 ms; `workflow-detail` green; `workflow-marks` fails only at its
  `shot` line 137, which is the expected headless refusal. Regression tests:
  `screens::board::actions::tests::{the_move_confirm_is_asked_when_the_mirror_holds_a_live_child_for_the_card,
  a_card_whose_run_has_ended_moves_with_no_confirm}` and
  `screens::board::runs::tests::cancel_takes_a_live_child_the_board_has_not_recorded_yet`; the
  first fails on the parent with the old `latest_run.id` gate restored.
- **OPEN, and now a scenario bug rather than an app one: `workflow-chain:113`, `workflow-chain:164`
  and `workflow-marks:108` await `working` on a child that is only working for two milliseconds.**
  The Claude gate child opens its gate 2–12 ms after `turn_started` (`agent_events` seq 4 → 14:
  1790034171140 → 1790034171142 in `/tmp/fleet-harness/20260921-234250-workflow-chain`), and the
  shell batches a bridge burst into one notify (`shell/root/events.rs`, `EVENT_BATCH_LIMIT`), so
  `Running` and `Blocked` usually arrive in the same projection and the tile goes straight to
  `needs you`. Those lines therefore pass in about half of all runs: measured 4/6 at ≤30 ms and
  2/6 at exactly 20.00 s — the gate budget, where what they finally catch is the `Settling` blip
  after the child gives up, not the run starting. Before the fix they were reliable only because
  the face was wrong. The exact change owed, which is a strengthening rather than a relaxation —
  `needs you` is what BOARD.md §11.8 says a parked child draws, and the tracker's own note said no
  scenario could await it until symptom (1) was fixed:
  `workflow-chain:113` → `await lists["board.cards"].rows[0].marks[0] == "needs you" 60000`;
  `workflow-chain:164` → the same word at its 90000 timeout; `workflow-marks:108` → the same word
  at its 30000 timeout; and the comment blocks above each (chain 101-110, marks 111-113) rewritten,
  because they now state the opposite of what the build does. The `agents.delegations[N].status ==
  blocked` awaits that follow them can stay or go — they are no longer standing in for a mark the
  face refuses to draw. **Owner: `corpus-board`.**
- **`hub/jobs-panel.scenario` lines 28 and 40 are wrong about the panel, and nothing on this
  machine had ever reached them: they sit after the file's `shot`, and a failed line ends the
  scenario.** Re-run with `--continue-on-failure`, both assert
  `lists.jobs.selected.label ~= "^Create acme/api#injected-[01]$"` for the row under the cursor
  after one `j`, and the row there is `Prepare copies for acme/api`. The panel is right and the
  assertion over-specifies: creating a worktree files a prepare-copies job of its own, so the two
  injected creates are not adjacent — the dump reads `Create acme/api#injected-0`, `Prepare
  copies for acme/api`, `Create acme/api#injected-1`, `Prepare copies for acme/api`, `Inspect
  worktrees`. The line's stated intent is that the cursor and the selected projected row move
  together, which says nothing about *which* job is second, so the fix is a rewrite of what it
  asserts rather than a relaxed regex — a Hub-corpus decision, not this task's, and one no
  baseline here can check. **Owner: whoever owns `scenarios/hub/`.**
- **`harness_headless` inside `cargo test -p fleet-app` is over its own budget and is now the
  round's main source of noise — the harness says so itself.** Measured 2026-09-21: `headless
  subset: 30 scenario(s) in 250.6s` and then `warning: the headless subset now costs make test
  251s, over its 60s budget; move scenarios back to 'make harness-headless'`. Two runs of that
  one test produced two *different* failure sets — run 1:
  `agents/subagent-reopen-closed-caller` line 13 and `board/workflow-chain` line 78; run 2:
  `agents/composer-editing` line 22 (`mode == Agent` reading `Terminal`),
  `agents/subagent-reopen-closed-caller` line 13 again, `board/card-create-editing` line 23 and
  `board/filter-columns` line 22 (both `lists.board…` *missing from the snapshot*, which is the
  board view being momentarily absent while it reloads). Every one of them is green run alone,
  and `make harness-headless` — the same 30 files with nothing else on the box — is green end to
  end. `card-create-editing` line 23 failed this way in the first full-corpus run of the day too,
  before any change in this task, so the class predates the round; the round made it louder by
  adding five scenarios to the subset. The decision the warning asks for — which of these belong
  in `make test` at all — is the corpus owner's, not this task's.
- **`board/workflow-chain.scenario` line 78 is a load flake, measured 2026-09-21.** Inside
  `cargo test -p fleet-app`'s `harness_headless` — the whole workspace suite on the same box —
  `key l` after `await key_contexts[2] == "Board"` left the selection on `Backlog` and the 10 s
  `await lists.board.selected.label == "Todo"` timed out; the key was dropped rather than slow.
  Green alone (and inside `make harness-headless`, which runs the same 30 files unloaded). The
  Board context arriving is not the same fact as the pane being ready for a bare letter, so the
  fix is an `await idle` before the key rather than a bigger budget.
- **`focused` can still say `agents.composer` when the composer does not have the keyboard.**
  It is `AgentThreads::composer_focused`, app state kept in step by a focus-in/out pair on the
  editor's handle, rather than a read of the focused handle at projection time — and while the
  `agents.composer` target covered the whole composer stack, a click into the metadata strip left
  it reading `agents.composer` with every keystroke going nowhere. The target fix removes the way
  in that a scenario has; the field can still be made to lie by any other route to the tab's
  fallback handle. Whoever owns `state/harness/projection.rs` should consider deriving it from
  the window's focused handle, which is what `record_composer_focus` already reads per frame.
- **`ctrl-s b` flashes `Terminal` for one frame while it creates the board tab.**
  `select_terminal` reads the tab's kind from the session record it already has, and on the
  *create* path that record has not arrived, so `native` is `false` and the optimistic write says
  `Terminal` over a board pane that is already drawn — the one thing the comment above that write
  says it exists to prevent. The next snapshot's `sync_terminal_mode` corrects it in about 14 ms,
  which is why `workspace/board-tab.scenario` now awaits the word rather than asserting it. The
  reply carries the `Terminal` record, so the fix is to let the create path say which kind it got.
- **Three load flakes measured in this round's full-corpus run**, each green when re-run alone:
  `board/navigation.scenario` line 22 (`await focused == board.column[0].card[1]` timing out at
  `card[0]`), `board/card-create-editing.scenario` line 23 (`lists.board.rows[0].badges[0]`
  missing from the snapshot) and `agents/subagent-attach-from-picker.scenario` line 10
  (`focused == agents.composer` reading `tabs.tab[0]`). All three are the §9.3 shape: an `assert`
  or a short `await` on a field that is still settling behind a different one.
- **A third load flake, newly observed at integration and not on the round's known list:**
  `fleet-app`'s `harness_headless::every_scenario_the_headless_lane_can_honour_passes`, failing
  inside `scenarios/agents/closed-tab-survives-a-daemon-restart.scenario` — either
  `await daemon.link == reconnected 60000` timing out with the link still `lost`, or
  `agents.threads[0].attached == false` still true at line 18. The binary is green run alone
  (twice, 2/2 in ~162 s); it fails when the whole workspace suite and a `cargo build` are
  competing for the machine. The scenario's own 60-second reconnect budget is the thing to
  revisit — it is a wall-clock budget in a lane that is supposed to be deterministic
  (`docs/TESTING-HARNESS.md` §9.3), and every other step in it is an `await` on a snapshot field.
- ~~**The four `scenarios/board/workflow-*.scenario` files are the feature's one remaining
  gap**~~ — all five are written and green (`workflow-marks`, `workflow-detail`,
  `workflow-columns`, `workflow-columns-context`, `workflow-chain`); only `workflow-marks` takes
  a `shot` and so sits out the headless lane.
- ~~P9-T05's owner: the scenario is where the confirm is proven end to end~~ — done 2026-09-21:
  `workflow-chain` reads both `MoveCancelsRun` sentences and the card-run cancel's back through
  the new `dialog.message`. The confirm *titles* are still unasserted by any GUI test: the
  snapshot names a dialog by its key context (`dialog.name == "Confirm"`) and carries no title,
  so `Cancel {KEY}'s run?` and `Move {KEY}?` remain pinned by unit tests alone. That is a
  deliberate stop, not an omission — §3.8.3 has the user accept the *sentence*, which is what
  `message` carries, and a second string in the snapshot would need its own reason to exist.
- **What `workflow-chain` covers and what it cannot, for whoever reads it next.** Covered: every
  phase-9 key, both its refusals and its confirms, against real children. Not covered, and each
  for a measured reason rather than an oversight:
  * **which of the two asked for the run `>` is followed by.** A column whose run ends restarted
    it immediately and for ever (phase 6's finding), so a run appearing after `>` was not
    attributable to `>`. The scenario says so at the line and pins the refusal instead. That
    restart is fixed as of 2026-09-21 (review fix F1): a terminal run no longer re-seeds its own
    column, so the attribution is now available to whoever next revisits this scenario.
  * **a second *live* run.** `maxLiveRuns` is 1 and the churning card wins the freed slot, so
    the second card is given its run only after the first has left its action column.
  * **`done`.** Both action columns carry an `on_success`, so a succeeded run is announced by
    the card moving; `state::board`'s unit test is where that mark lives.
  * **a parked child held longer than five seconds.** `agent::GATE_BUDGET` is the ceiling on
    every state a scripted child can hold, which is why each move confirm is pressed at the
    start of a `needs you` window rather than wherever the scenario happens to arrive.
- ~~**`X` on a card that is only *owed* a run is a contract gap.**~~ **Fixed 2026-09-21 (review
  fix F2/K):** the daemon drops the reservation, which is the reading `docs/UX-SPEC.md` § Board
  and the app's own consequence line already promised. `Boards::cancel_run` with no live run now
  clears `pending_run`, writes `Run canceled: it was still waiting for a slot` (§11.3) and saves;
  `{KEY} has no live run` is kept for a card with neither. Pinned by
  `services::boards::tests::triggers::{cancelling_a_waiting_card_drops_the_run_it_was_owed,
  cancelling_a_card_with_nothing_running_is_refused}`. The original report follows.

  **`X` on a card that is only *owed* a run is a contract gap, found by reading the code around
  this scenario rather than by running it.** The app treats a `pending_run` as cancellable —
  `runs::RunTarget.live` is `pending_run.is_some() || latest_run.is_live()`, and
  `actions::card_run_cancel_draft` writes the risk line `{KEY} is waiting for a free slot; the
  run it is owed is dropped` — but the daemon's `Boards::cancel_run` asks `live_run(card)`,
  which reads the newest *run row* only, so a pending-only card is answered `NotFound: {KEY} has
  no live run` (contracts line 537) and the sentence lands in the sticky slot after the user has
  already confirmed. Either the daemon should drop the reservation or the app should refuse the
  key; §5.5 can be read both ways, so it is a stop-and-ask rather than a fix to make inside a
  scenario task. **Owner: whoever owns the automation engine (phase 3).** The scenario
  deliberately cancels a *live* run, which is the path both ends agree on.
- ~~P9-T01's owner: the five stub handlers carry no refusal toast yet~~ — done 2026-09-21; no
  `CONTRACT STUB` is left in `fleet-app`.
- ~~**P9-T02's remainder, for whoever owns `screens/workspace/agent.rs` (an integrator):** three
  edits, none in a P9-T02 file.~~ — done 2026-09-21 at integration, as described under P9-T02.
  The recipe below is kept only as the record of what was asked for.
  1. `screens/agent_thread/mod.rs`: field `card_caller: Option<presentation::CardCaller>` on
     `AgentThreadView`, `card_caller: None` in `new`.
  2. `screens/agent_thread/sync.rs`: `pub(crate) fn sync_card_caller(&mut self, caller:
     Option<presentation::CardCaller>, cx: &mut Context<Self>)` shaped exactly like
     `sync_caller` (early return on equality, then `refresh_metadata`, `sync_composer`,
     `cx.notify`); in `refresh_metadata`, before the thread-caller push, `if let Some(card) =
     &self.card_caller { metadata.push(presentation::card_metadata_segment(card)); }`; in
     `sync_composer`'s placeholder match, an arm before the child one: `None if matches!(mode,
     ComposerMode::Normal) && self.card_caller.is_some() =>
     presentation::CARD_COMPOSER_PLACEHOLDER.to_owned(),`.
  3. `screens/workspace/agent.rs`: beside `caller_context`, add

     ```rust
     fn card_caller_context(app: &AppState, child: ThreadId) -> Option<presentation::CardCaller> {
         let delegation = app.agents.delegation_of_child(child)?;
         let (board, card) = delegation.caller.card()?;
         let view = app.board().filter(|view| &view.board.id == board)?;
         let card = view.cards.iter().find(|candidate| &candidate.id == card)?;
         let column = view.board.statuses.iter()
             .find(|status| status.id == card.status_id)
             .map_or_else(String::new, |status| status.name.clone());
         Some(presentation::CardCaller {
             key: card.display_key(&view.board),
             column,
             board: view.board.name.clone(),
         })
     }
     ```

     read it in the same `state.update` block that reads `caller_context`, and call
     `view.sync_card_caller(card_caller, cx);` beside `view.sync_caller(...)`. Deleting the two
     `#[cfg_attr(not(test), expect(dead_code, …))]` attributes in `presentation.rs` is the last
     step — the unfulfilled expectation is the signal.
- ~~**`^s u` from a card run** (`screens/workspace/actions.rs`, also not a P9-T02 file)~~ —
  done 2026-09-21 at integration: `up_to_card_caller` is the second answer, `jump_to_card` is the
  jump, and the segment carries its `card:` target again. Kept as the record of what was asked
  for: today `up_to_caller` answers `agents.caller_of(child)`, which is `None` for a card-called
  child, so the key toasts `^s u is not bound here`. It needs a second answer — the board and card from
  `agents.delegation_of_child(child)?.caller.card()` — and the handler a second branch that
  selects the worktree's `fleet://board` tab and calls `screens::board::focus_card`. Once that
  lands, give the segment its target back: `.target(format!("card:{board}/{card}"))` in
  `presentation::card_metadata_segment`, and parse that prefix in
  `screens/agent_thread/view.rs`'s `on_target` into a new `AgentThreadEvent::SelectCard`.
