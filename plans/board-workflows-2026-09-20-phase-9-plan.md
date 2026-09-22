# Board workflows, phase 9: app and docs — keys, palette, confirm, end-to-end scenario — Plan
> Tracker: ./board-workflows-2026-09-20-phase-9-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

The last phase makes the face and the detail act: `A` attaches the focused card's run as an ordinary
tab, `X` cancels it after a confirm, `>` starts or re-starts the column's action, `b` opens the
blocked-by picker and `m` the agent picker; each gets a palette command, and phase 8's `C` the
sixth. `[` and `]` ask `{KEY} is working ({elapsed}). Move to {target} and cancel the run?` and on
yes send `MoveCard { cancel_run: true }`. A refused start raises the sticky error from the run's
detail, the attached thread says what it works for in its metadata segment and placeholder, and `^s
u` returns to the board; one scenario then does the story end to end, and an audit closes it.

## Sizing call

**Phased, phase 9 of 9.** See ./board-workflows-2026-09-20-roadmap.md. One pull request, last,
depending on phases 6, 7 and 8. Not split further: a key, its handler, its palette command and its
document row are one change, and keys without the scenario leave the initiative unverified.

## Repository context

- Actions `actions.rs:732-787` (`board`), `card_detail` :790-821; handlers `shell/root/board.rs`
  (one fn per action, card-detail :209-317), registered `shell/root/actions.rs:268-302`.
- Keymap `keymap.rs`: `Hub > Board` :443-469, `Workspace > Native > Board` :474-499, `Dialog >
  CardDetail` :500-518, registry :999-1040. Occupancy (research E §3): `A`, `X`, `>`, `b`, `m`, `C`
  free on both board contexts; outer `Hub` binds `A` :571 and `b` :577, which the board contexts
  shadow while they own the keys — `APP-CONTRACTS.md` §6 records it.
- Confirm `dialogs/confirm/policy.rs`: `ConfirmRequest` :8-70, `title` :80-93, `consequence`
  :97-149, `icon` :153-166, `action_label` :170-180, `rechecks` :193-195, `target` :199-209;
  `DelegationCancelDraft` `confirm.rs:43-48`, `PendingDelegationCancels` :51-53. Move path
  `screens/board/actions.rs:205-231`, `toast_readonly` :125, `lifecycle::fail` :224-233.
- Palette `dialogs/palette.rs`: `Command::Board*` :182-224, labels :369-391, action names :511-533,
  `Command::ALL` :304-305, `valid_with` :588-622 (`on_board` :594), `run_command` :1495 (board arms
  from :1518); `board_pane_is_active()` `state/board.rs:344-352`; picker rows :853-883.
- Attach `bridge.rs:186` (`AgentThreadOpen`, body :305), callers
  `screens/workspace/agent/requests.rs:96`/:158; chrome `screens/agent_thread/presentation.rs`
  `caller_metadata_segment` :110-118, `child_composer_placeholder` :123-126, applied
  `mod.rs:258-261`. Wire (phase 3): `CardRunStart`, `CardRunCancel`, `MoveCard { cancel_run }`.
- Skills: `gpui-app-shell` (actions, keys, confirm, palette), `gpui-state-and-memory` (the
  pending-cancel state), `rust-async-background-work` (every request is a spawned task ending in
  `.detach_and_log_err(cx)`), `rust-gpui-testing` with `TESTING-HARNESS.md` §3/§9,
  `zed-quality-review` last.

## Assumptions

- **No new request shapes.** Everything sends what phase 3 serves; a daemon without
  `board.automation` answers the capability error, shown verbatim, and `A` on a finished run opens
  its transcript as a finished child does. The confirm is a courtesy: the daemon still refuses an
  unconfirmed move with `{KEY} is working; pass --cancel-run to move it`, shown via `fail`.

## Out of scope

- Any daemon change; the ui-kit (6 and 7 own it); a second window.

## Affected areas

- `crates/fleet-app/src/{actions.rs, keymap.rs, shell/root/{board.rs, actions.rs},
  screens/board/{actions.rs, lifecycle.rs}, screens/agent_thread/presentation.rs,
  dialogs/{confirm.rs, confirm/policy.rs, palette.rs}}` and their tests;
  `scenarios/board/workflow-chain.scenario`; `docs/{KEYMAP.md, APP-CONTRACTS.md, NATIVE-AGENTS.md,
  UX-SPEC.md, BOARD.md, TESTING-HARNESS.md}`, `.claude/skills/fleet-{board-planning,subagent-cli}`.

## Tasks

### P9-T01 — Five actions, their keys and their handlers
- **Intent / touches:** Make the keys exist — `actions.rs:732-787`, `keymap.rs`,
  `shell/root/board.rs`, `shell/root/actions.rs`.
- **Steps:**
  - `board::{AttachRun, CancelRun, RunNow, PickBlockedBy, PickAgent}` beside today's board actions,
    one handler each in `shell/root/board.rs` shaped like `pick_*` :169-189, registered :268-302.
  - Keymap rows exactly as §5.5: `A`, `X`, `>` on `Workspace > Native > Board` and `Dialog >
    CardDetail` only — never on `Hub > Board`, where a context board can never hold a run and a key
    that can only refuse is an affordance that does nothing; `b` and `m` on all three.
  - Refusals are toasts, not silence: `A` with no run says `{KEY} has no run`, `>` outside an action
    column says `{column name} has no action`, a read-only board keeps `toast_readonly`.
    `PickBlockedBy`/`PickAgent` open phase 7's `BlockedBy` and `Provider` kinds through
    `open_picker` (:83); `m` lands on Provider, Model and Effort being the next rows.
  - Tests: each action reaches its handler from both surfaces, each refusal toasts its sentence, and
    `A`/`X`/`>` are absent from `Hub > Board` in `keymap::table()`.
  - Draw the run row's hint line `A attach · X cancel · > re-run` under phase 7's run row, and
    extend the card-detail dialog hint line (`card_detail/view.rs:103-117`) with `A attach · X
    cancel · > run` before `esc close`, in this phase only, so no affordance precedes its key
    (contracts §5.3).
- **Verification / done when:** `cargo test -p fleet-app shell::root::board`, `make lint`; every key
  in §5.5's table is bound where the table says and nowhere else.

### P9-T02 — Attach, and what the attached thread says
- **Intent / touches:** The run's thread as an ordinary tab — `shell/root/board.rs`,
  `screens/agent_thread/presentation.rs`, `screens/workspace/agent/requests.rs`.
- **Steps:**
  - `AttachRun` resolves the focused card's live run, else its newest, takes `CardRun.thread_id` and
    sends `AgentThreadOpen` (`bridge.rs:186`) through `requests.rs:96`/:158; the spawned task ends
    in `.detach_and_log_err(cx)`. From the detail it closes first.
  - Pinned metadata: a non-collapsible leading segment `for {KEY} · {column name} · {board name}`
    for a thread whose caller is a card, beside `caller_metadata_segment` (:110-118), its `target`
    selecting the board tab with that card focused — the jump `^s u` makes. Composer placeholder
    `Steering a card run. Its report moves the card when it finishes.` beside
    `child_composer_placeholder` (:123-126), applied at `mod.rs:258-261`.
  - Tests: the segment for a card caller and today's text for a thread caller; the placeholder; `^s
    u` selects the board tab and focuses the card.
- **Verification / done when:** `cargo test -p fleet-app agent_thread`, `make lint`; a card run's
  tab differs from any other agent tab only in those two strings.

### P9-T03 — Cancel with a confirm, `MoveCancelsRun`, and the sticky error
- **Intent / touches:** The three destructive paths — `dialogs/confirm{/policy.rs,.rs}`,
  `screens/board/{actions.rs, lifecycle.rs}`.
- **Steps:**
  - `X` reuses the delegation-cancel pattern (`DelegationCancelDraft`/`PendingDelegationCancels`,
    `confirm.rs:43-53`): confirm, then `CardRunCancel { card_id }`; a card whose marks show no live
    run toasts `{KEY} has no live run` instead (contracts §5.5).
  - `ConfirmRequest::MoveCancelsRun { card, key, target, elapsed }` in `policy.rs:8-70`, title `Move
    {KEY}?`, consequence `{KEY} is working ({elapsed}). Move to {target} and cancel the run?`, plus
    `icon`, `action_label`, `rechecks` and `target` arms — the enum is closed, so budget the
    exhaustive-match sweep.
  - `fn move_card` (`screens/board/actions.rs:205-231`): when the focused card has a live or pending
    run **and** the delegation mirror holds a live card-called delegation for `latest_run.id`
    (`elapsed` from `Delegation::elapsed`), request the confirm instead of sending, and on yes send
    `MoveCard { cancel_run: true }`; otherwise send what it sends today (contracts §5.5). A daemon
    `Conflict` still reaches the user through `lifecycle::fail` (:224-233).
  - Failed start: phase 6's `apply_board_view` rule (a new run id with `failed_to_start()`) already
    raises the sticky error; this phase only proves it in the scenario. Tests: the confirm's title and consequence; yes sends `cancel_run: true` and no
    sends nothing; a card without a run moves with no confirm; the sticky error fires once.
- **Verification / done when:** `cargo test -p fleet-app screens::board confirm`, `make lint`; no
  destructive path runs without a confirm or a daemon sentence on screen.

### P9-T04 — Six palette commands
- **Intent / touches:** Key/palette parity — `dialogs/palette.rs`.
- **Steps:**
  - Variants beside `Command::Board*` (:182-224): `BoardAttachRun`, `BoardCancelRun`, `BoardRunNow`,
    `BoardPickBlockedBy`, `BoardPickAgent`, `BoardColumns`; labels (:369-391) `Board: Attach run`,
    `Board: Cancel run`, `Board: Run now`, `Board: Blocked by`, `Board: Agent`, `Board: Columns`;
    names :511-533; all six in `ALL` (:304-305).
  - `valid_with` (:588-622): the three run commands are valid when `board_pane_is_active()` and a
    card is focused, or when `Dialog > CardDetail` is open over a `BoardScope::Worktree` board;
    never over the Hub's context board (contracts §5.5); `Blocked by`, `Agent`, `Columns` follow `on_board` (:594). `run_command`
    (:1495) dispatches to the P9-T01 handlers, never to a copy.
  - Tests: each label resolves to its action name, the three run commands are absent on the Hub
    board, and every command runs its key's code path.
- **Verification / done when:** `cargo test -p fleet-app palette`, `make lint`; every new key has
  one palette command and none is offered where its key is unbound.

### P9-T05 — `scenarios/board/workflow-chain.scenario`, end to end
- **Intent / touches:** The acceptance test — the scenario file, plus
  `state/harness/projection.rs` only if a field is missing.
- **Steps:**
  - On phase 6's `board-workflow` fixture, a scripted thread's `shell` steps (§5) run `fleet board
    card new`, `--blocked-by` and `card move` to build a three-card chain and put the first into
    Ready — the mechanism the `subagent-*` scenarios use to reach the CLI.
  - Then on the board tab: `await` the first card's `working` mark; `A`, asserting a new tab titled
    with `↳` and `focused == agents.composer`; `^s u` back, asserting the card is focused; `^s d`,
    asserting the run is a top-level AGENTS row labelled by its tab title (the no-work claim); `X`
    on the next card, confirm, await its run terminal; `>` and await a second run; finally `]` on a
    working card and assert the confirm's consequence text. Every step is an `await` on a snapshot
    field, never a `wait` (§9.3), and there is no `shot`.
- **Verification / done when:** `make harness-one SCENARIO=scenarios/board/workflow-chain.scenario`,
  then `LANE=headless`, then `make harness`; it passes in both lanes with no new command or target.

### P9-T06 — The four documents and the cross-document audit
- **Intent / touches:** Every authoritative document agreeing with shipped code — `docs/*` below,
  `docs/agents-contracts.md`, the two `.claude/skills/fleet-*` skills, `TODO.md` if it exists.
- **Steps:**
  - `KEYMAP.md`: rows for `A`, `X`, `>`, `b`, `m` in both board contexts and the detail, matching
    `keymap::table()`, plus the sentence that `b` shadows the Hub's open-in-browser while the board
    owns the keys. `APP-CONTRACTS.md`: the one-way sentence — the app never starts a run, it asks
    the daemon to change a card and draws what comes back — and the `b` shadow in the §6 arbitration
    list (:300-306, :558-576).
  - `NATIVE-AGENTS.md` §15.3 gains the sentence that a card is a caller like a thread, with the
    attach, cancel and steering rules that follow, and §2's mark vocabulary a cross-reference that
    the face and detail reuse it unchanged; `UX-SPEC.md`'s Board "Keyboard" table gains the five
    keys and the confirm sentence.
  - Audit each against the code, fixing what disagrees in this PR: `BOARD.md` §11,
    `NATIVE-AGENTS.md` §2/§15, `UX-SPEC.md` Board and §3.8.6, `KEYMAP.md`, `APP-CONTRACTS.md`,
    `TESTING-HARNESS.md` §3/§4/§5, `agents-contracts.md`, the two skills, `TODO.md`; confirm §2's
    grammar and every pre-existing target name are byte-identical to `main` but for the agreed
    preset enumeration. Record in the tracker's follow-ups that **no phase ran a real provider** — a
    manual smoke against `claude` and `codex` on a real worktree board (preset, chain, attach,
    cancel, re-run) is owed before the feature is announced.
- **Verification / done when:** `git diff docs/` beside `keymap::table()` and the scenarios, and `rg
  -n "cancel_run|CardRunStart|board::AttachRun" docs/` finds a sentence for every surface.

## Verification

```sh
make lint && make test && make harness
```

`make harness` is required: this phase changes the keymap and two screens. Run
`scenarios/board/workflow-chain.scenario` first, then the corpus.

## Definition of done

- [ ] Every task is `[x]`; `make lint` and `cargo check --workspace --all-targets` are clean and
      `make test` passes.
- [ ] `make harness` passes with `workflow-chain.scenario` and every earlier workflow scenario;
      `KEYMAP.md` matches `keymap::table()` row for row and `APP-CONTRACTS.md`, `NATIVE-AGENTS.md`,
      `UX-SPEC.md`, `BOARD.md`, `TESTING-HARNESS.md` agree with the code.
- [ ] Every spawned request ends in `.detach_and_log_err(cx)` or is stored, no destructive path
      lacks a confirm or a sentence, and the tracker names the real-provider smoke as a follow-up.

## Risks and rollback

- **Closed enums break the workspace.** New `ConfirmRequest` and `Command` variants fail every
  exhaustive match at once under `-D warnings`; sweep first, never with a catch-all arm.
- **Two cancel paths.** `X` and a confirmed move must share one request and one pending-cancel set,
  or a double confirm appears. If the long scenario turns flaky, split it rather than adding `wait`.
- **Rollback.** Revert the PR: phases 6 to 8 still draw the state and the CLI still drives the
  workflow, so the app degrades to read-only.

## Contract resolutions

Resolved in the contracts file on 2026-09-20; the contracts wording wins over any earlier phrasing above.

1. The move confirm is asked only when the app's delegation mirror holds a live card-called delegation for `latest_run.id`; `elapsed` comes from `Delegation::elapsed`; otherwise the plain move is sent and a daemon `Conflict` is reported through `lifecycle::fail` (contracts §5.5).
2. The failed-start sticky error is phase 6's `apply_board_view` rule over `CardRun::failed_to_start()` (`thread_id: None`); phase 9 only proves it in the scenario (contracts §1.3, §5.2).
3. `X` on a card whose marks show no live run toasts `{KEY} has no live run`, the daemon's own `NotFound` sentence (contracts §5.5).
4. `Board: Attach run`, `Cancel run` and `Run now` are valid when `board_pane_is_active()` or when `Dialog > CardDetail` is open over a `BoardScope::Worktree` board; never over the Hub's context board (contracts §5.5).
5. The run row's hint line is drawn in this phase with the keys (contracts §5.3).
