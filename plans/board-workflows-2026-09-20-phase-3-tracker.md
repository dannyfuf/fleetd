# Board workflows, phase 3: the automation engine — Tracker
> Plan: ./board-workflows-2026-09-20-phase-3-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.
- Definition of done is not met until every box is ticked and this tracker matches reality.

## Kickoff
- [x] I have read the plan end to end.
- [x] I have run the project-wide verification commands once on a clean tree to confirm a green baseline.
- [x] I am ready to start.

## Tasks
- [x] P3-T01 — The pure engine and its table tests
  - Shapes by `contracts:foundation`: `board/automation.rs` declares `LiveIndex`, `StartRun`,
    `Plan`, `ResolvedPrefs` and the four functions with the contract's signatures, and `board.rs`
    re-exports them. `LiveIndex` and `resolve_prefs` were already real.
  - Closed by `d:core-engine`: `re_evaluate` (a private `Walk` holding the reservation, the `seen`
    set and the plan; rule 1 start-or-park, rule 2 `advance_when_unblocked`), `next_pending` and
    `brief` are implemented, every `// CONTRACT STUB (d:core-engine)` is gone, and the module
    imports nothing outside `fleet_core::board`.
  - verified: `cargo test -p fleet-core board::automation` (21 passed — start, throttle, park,
    reservation, diamond, two cycles, both `next_pending` orders, three brief tests),
    `cargo test -p fleet-core` (292+5+2 passed), `cargo clippy -p fleet-core --all-targets
    --all-features -- -D warnings` clean.
- [x] P3-T02 — `Automation`, `Boards::new` and the composition wiring
  - Skeleton by `contracts:daemon`: `boards/automation.rs` holds `Automation` (the four fields of
    §3.5) and the module doc's gate rule; `Boards::new` takes `Option<Automation>` and
    `Boards::automation()` serves the trigger sites; `composition.rs` passes `Some` iff
    `delegation::install` returned `Some` and installs the delivery hook as a `Weak`. Every verb
    body is `// CONTRACT STUB (d:boards-automation)` / `(d:resume)`.
  - Internals closed by `d:boards-automation`: every `// CONTRACT STUB (d:boards-automation)` is
    gone — the three run verbs, `start_for_card`, `apply_starts` and `on_run_delivered` are real,
    and `Boards::evaluate_with_reservation` is the one walk that holds the board's own reservation.
  - Closed by `d:resume`: `maintenance.rs` spawns `run_board_automation(services, shutdown)` beside
    the outbox worker — it subscribes to the bus *before* the sweep, calls `resume_automation` once,
    then hands the freed slot on for every terminal `DelegationChanged` (and on `Lagged`, which asks
    the memo instead of the event), under a `biased;` shutdown branch. `resume.rs` holds no
    `// CONTRACT STUB (d:resume)` any more.
  - verified: `cargo test -p fleet-daemon --lib services::composition` (5 passed, including
    `the_delegation_worker_is_composed_with_the_daemon_and_stops_with_it`), `cargo test -p
    fleet-daemon --lib services::maintenance` (11 passed).
  - verified: `cargo test -p fleet-daemon --lib services::boards` (18 passed), `cargo test -p
    fleet-daemon --test boards_service` (65 passed), `cargo check --workspace --all-targets` and
    `cargo clippy -p fleet-daemon --all-targets --all-features -- -D warnings` clean.
- [x] P3-T03 — The five trigger sites and the daemon refusals
  - Cards half done by `d:triggers-cards` (`boards/cards.rs`): `create_card` validates links and
    seeds the new card; `update_card` refuses to archive a working card, clears a `pending_run` and
    seeds on a `status_id` or `archived` change, and validates links after the patch; `move_card`
    refuses a working card without `cancel_run` and cancels first with it, clears the marker
    silently and seeds; `delete_card` refuses a working card, drops every link to it with
    `Unblocked: {KEY} was deleted` and seeds the cards it freed. All four go through one private
    `Boards::commit` — `evaluate_with_reservation`, save once, refresh the pending memo, drop the
    gate, `apply_starts` — and share `is_working`, which the run verbs read too.
  - verified: `cargo test -p fleet-daemon --lib services::boards` — 50 passed, 8 of them the new
    `services::boards::tests::triggers::*` (the created start, the edit that seeds nothing, the
    move that starts and the move that parks behind the ceiling, the two `--cancel-run` halves,
    the pending marker cleared silently, the archive and delete refusals, the dropped links);
    `cargo test -p fleet-daemon --test boards_service` (65 passed) and `--test boards_jira`
    (35 passed).
  - Lifecycle half closed by `d:lifecycle` (`boards/lifecycle.rs`, `boards/sync.rs`): `update`
    captures the columns as they were, seeds every card whose column changed category, evaluates
    once before its single save, then notes the pending memo, drops the gate and applies the
    starts. A column that lost `on_enter` (or that the patch removed) clears the parks waiting on
    it with `Run canceled: {column} no longer runs an action`; a column a live run still names is
    `Conflict` `column has {n} live runs; cancel them first`, refused ahead of the in-use check so
    the runs and not the cards are what is reported. The three §1.7 `automation` sentences are one
    new `Boards::require_automatable`, run only when the patch *asks* for automation. `sync.rs`
    says in a comment why a pull evaluates nothing.
  - `max_live_runs` needed no daemon code: phase 1's `validate_automation` already refuses
    `must be between 1 and 8`, and `apply_board_patch` runs it on every `update`. Covered by a test.
  - verified: `cargo test -p fleet-daemon --lib services::boards` — 54 passed, 6 of them the new
    `services::boards::tests::refusals::*`; `cargo test -p fleet-daemon --test boards_service`
    (65 passed), `cargo test -p fleet-core board` (183 passed), `cargo clippy -p fleet-daemon
    --all-targets --all-features -- -D warnings` clean.
- [x] P3-T04 — `start_for_card` and `on_run_delivered`
  - `d:boards-automation`: `start_for_card` reads the document outside every gate, resolves the
    prefs, builds the brief, the templated column environment and the child title, calls
    `run_for_card`, then re-acquires the gate to push either the started `CardRun` or the
    failed-to-start one carrying the refusal's sentence — clearing `pending_run`, refreshing the
    pending memo and releasing the reservation on both paths. `on_run_delivered` reads the usage
    before the gate, is idempotent by delegation id, writes `ended_at`, the outcome table,
    `detail`, `files_changed = 0` and the cost, appends the capped report excerpt (rotating at
    `MAX_REPORT_COMMENTS_PER_CARD` and clearing the run that owned a dropped one), pushes
    `RunEnded`, applies the `on_success` move as an `AutoMoved`, re-evaluates, saves once, drops
    the gate and applies the starts.
  - verified: `cargo test -p fleet-daemon --lib services::boards::automation` — 16 passed (the
    outcome table including `reported blocked`, the non-terminal refusal, the `RunEnded` sentence,
    the excerpt cap on a character boundary, the report rotation, the run cap, both `AutoMoved`
    swaps, the failed-to-start row, the model sentinel, the templated column environment and its
    `FLEET_` refusal, the worktree-board sentence).
- [x] P3-T05 — Run verbs, `resume_automation`, the slot subscriber, the read joins
  - Verbs done by `d:boards-automation`: `start_run` refuses `{column name} has no action` and
    `{KEY} is working; cancel the run first`, then seeds one evaluation; `cancel_run` refuses
    `{KEY} has no live run` and reaches the delegation service with no gate held, leaving the
    outcome to `on_run_delivered`; `wait_run` answers a terminal newest run at once and otherwise
    waits on `delegations.wait(id, timeout_ms, None)` before re-reading the card.
  - Boot recovery and the freed slot done by `d:resume` (`boards/automation/resume.rs`):
    `resume_automation` sweeps every worktree board — reading the delegation service *before* each
    gate — and per board adopts a live delegation a row already names, records a terminal one
    nothing will deliver again through `on_run_delivered` itself, closes `Incomplete` a row whose
    delegation is gone, writes the row for a live delegation no row names (the crash window between
    `run_for_card` and `record_run`), then evaluates once from the cards the board still *owes* a
    run and applies the starts outside the gate. `on_slot_released` copies `pending_boards`, drops
    the guard, and per board takes `next_pending` under the gate, re-evaluates and leaves the memo
    when nothing is parked. Every board is swept even when one fails; the first failure is returned.
  - verified: `cargo test -p fleet-daemon --lib services::boards::automation::resume` — 11 passed
    (the disposition table over the delivery states, the lost-start rule and the elapsed clamp);
    `cargo test -p fleet-daemon --lib services::boards` (65 passed), `cargo clippy -p fleet-daemon
    --all-targets --all-features -- -D warnings` clean.
  - Read joins, by `d:lifecycle`: `ops::summarize` already takes `live: &[LiveRun]` and `now` and
    fills `working_count`/`attention_count` — phase 1 landed that, and `lifecycle.rs`'s two
    `summarize` calls already pass `&self.now()`, so the summaries memo stays a memo and takes no
    delegation read. The `BoardView.live_runs` join in `get`, `ensure` and `ensure_for_worktree`
    is done at integration: `Boards::live_runs(board)` (`boards/automation.rs`) forwards to
    `DelegationService::live_for_board` and maps each card caller through `card_live_run`, and
    `get` calls it after dropping the card-index guard. `ensure`/`ensure_for_worktree` keep
    `live_runs: Vec::new()` on the *creating* path alone — a board created by that call has no
    cards — and every existing-board path there returns through `get`, which joins.
  - verified: `cargo test -p fleet-daemon --lib services::boards` — 50 passed, including
    `refusals::the_run_verbs_refuse_on_a_daemon_with_no_native_agent_database`.
  - verified (integration, 2026-09-21): `cargo test --workspace` and
    `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean with the join in.
- [x] P3-T06 — The wire, the client and the capability
  - Done by `contracts:foundation`: `MoveCard.cancel_run`, `CardRunStart`, `CardRunCancel`,
    `CardRunWait`, their goldens (plus a `BoardView` with a `live_runs` entry), the three
    `classify.rs` arms with a router test, the `translate.rs` arms, and the whole client side —
    `card_run_start`, `card_run_cancel`, `card_run_wait`, `move_card(.., cancel_run)`,
    `required_capability`, `capability_error` and the three `request_timeout` decisions, with tests.
  - Closed by `contracts:daemon`: the three `dispatch.rs` arms now reach `Boards::{start_run,
    cancel_run, wait_run}` and `MoveCard` forwards `cancel_run`; `BOARD_AUTOMATION_CAPABILITY` is
    advertised at `server/connection.rs` beside `board.worktree`, in both the live chain and its
    literal test mirror. Caveat for P3-T04/T05: the verbs are *routed* but not yet *implemented*,
    so until `d:boards-automation` lands, a peer that reads the capability and calls one gets
    `Unsupported("not implemented")`. That is the one window where `rust-ipc-protocol` Rule 7 is
    bent, and it closes with the real bodies — do not ship the branch with it open.
  - verified: `cargo test -p fleet-proto` (all green, every pre-existing golden byte-identical),
    `cargo test -p fleet-client` (all green), `cargo test -p fleet-daemon --lib router` (22 passed).
- [x] P3-T07 — The end-to-end suite and the docs
  - `crates/fleet-daemon/tests/boards_automation.rs` drives a private `fleetd` over its socket
    against a scripted Codex. `fleet-harness`'s `Preset::BoardWorkflow` builds the world — real git
    origins, two published worktrees, the `gh`/`acli`/`fleet` shims — and each test then builds its
    own board on the worktree the preset leaves bare, so nothing a test asserts was seeded by
    something else. The child transcript is the file's own (no `sleep 5`), installed over the
    preset's Codex shim through `HarnessEnv::install_fake`.
  - `a_four_card_diamond_never_runs_two_delegations_at_once`: A blocks B and C, B and C block D,
    one live run allowed, all four created in the routing column and only A moved by hand. The
    board drives the other three, the poll asserts the ceiling at every sample, and the persisted
    rows are then checked for four `Succeeded` runs on Codex, no overlapping interval, and A first
    and D last.
  - `a_restart_mid_run_adopts_the_live_delegation`: a child that is still working when its daemon
    is shut down; the replacement's boot sweep keeps the same run id, still live, in the same
    column — not a second run and not an `Incomplete` close.
  - `automation_is_refused_on_a_context_board_a_jira_board_and_a_hosted_worktree`: the three §1.7
    sentences, verbatim, off the wire.
  - `docs/BOARD.md` gained §4.1 (the seven triggers, the plan-outside-the-gate rule, the refusal
    table), the `board.automation` half of §5 with the three requests and their deadlines, the
    three client methods in §6, and §11.7 (entry loop, reservation, throttle order, outcome table,
    restart rules) under a rewritten §11 lead-in. `docs/NATIVE-AGENTS.md` §15.7 already carried the
    `Deliver`-row sentence and needed no edit.
  - verified: `cargo test -p fleet-daemon --test boards_automation` — 3 passed, twice in a row;
    `FLEET_DAEMON=… cargo test -p fleet-daemon --no-fail-fast` — 918 lib tests and all 29
    integration binaries green, 0 failed; `cargo test -p fleet-core` (292+5+2 passed),
    `cargo test -p fleet-proto` (43+25+14+1 passed), `cargo clippy -p fleet-daemon -p fleet-core
    -p fleet-proto --all-targets --all-features -- -D warnings` clean.
  - Note for whoever runs the daemon suite by hand: `cargo test -p fleet-daemon` without
    `FLEET_DAEMON` fails five pre-existing tests in `server::connection`, `services::sessions` and
    `services::sleep` — they build a `Sessions` in process and cannot start a PTY holder without it
    (`docs/DEVELOPMENT.md`). All five pass with it set, and `make test` exports it.

## Notes / decisions log
- 2026-09-21 (review fixes) — four daemon fixes from the three-reviewer pass, all with
  regression tests in `services/boards/tests/triggers.rs` and the docs they needed:
  1. **F1, P0.** `on_run_delivered` seeded `re_evaluate` with the delivered card whatever the
     outcome, so a run that ended where it started restarted its own column at once and for
     ever — measured at 20 runs and 20 child processes a minute, a cancel followed by a
     replacement run, and one failing card holding a `max_live_runs = 1` board for ever. The
     card is now seeded as *entered* only when `move_on_success` actually moved it (it returns
     `bool` for that), and as *settled* otherwise, through the new
     `fleet_core::board::re_evaluate_settled`. `docs/BOARD.md` §11.7 and contracts §2/§3.5 say
     so.
  2. **F2, P1.** The reservation was one daemon-wide `BTreeSet<CardId>` counted against
     `settings.max_live_runs`, which is *per board*: a card reserved on one board parked a card
     on another, and only some delegation somewhere going terminal would have released it. It is
     now `BTreeMap<BoardId, BTreeSet<CardId>>`, entered and dropped per board.
  3. **K, P1.** `cancel_run` refused a card that was only *owed* a run, after the app had
     already asked the user to confirm dropping it (UX-SPEC § Board). It now clears the
     `pending_run` and writes `Run canceled: it was still waiting for a slot`; `{KEY} has no
     live run` is kept for a card with neither.
  4. **H, P1.** `move_card --cancel-run` drops the board gate to cancel and did not re-check on
     the way back in. It now compares run ids and refuses when a *different* live run appeared
     in that window, rather than moving a card out from under a child nobody cancelled.
- 2026-09-21 (contracts:foundation) — phase 1 had already landed contracts §1 (the whole board
  automation model, `LiveRun`/`live_runs`, the automated-board goldens) and
  `BOARD_AUTOMATION_CAPABILITY`. Nothing of that was rewritten; the new goldens sit beside the
  phase-1 ones and the phase-1 fixtures are untouched.
- 2026-09-21 (contracts:foundation) — the client's capability gate answered every refusal with the
  worktree-board sentence. It now maps the capability to its own sentence through `capability_error`,
  so board automation says "this daemon does not support board automation; run `fleet daemon
  restart`" (contracts §3.6) and worktree boards keep theirs.
- 2026-09-21 (contracts:daemon) — `Automation` is `pub` and re-exported from `services/boards.rs`,
  not `pub(crate)` as §3.5 writes it. `Boards::new` is `pub` and called from two integration tests,
  so a `pub(crate)` parameter type is `private_interfaces`, denied by `-D warnings`. Every field
  stays private and `Automation::new` stays `pub(crate)`, so nothing outside the crate can build one.
- 2026-09-21 (contracts:daemon) — `in_flight` and `pending_boards` are `tokio::sync::Mutex`, like
  the board `gates` beside them: `on_slot_released` walks the memo while awaiting a board gate, and
  a `std` guard held across that await would be a deadlock waiting to be written.
- 2026-09-21 (contracts:daemon) — `DelegationService::live_for_board`/`live_for_card` read through
  `delegation_write`, as `run.rs`'s `caller_exists` already does, because the reader pool has no
  typed card seam. A board read therefore queues behind the writer; if that shows up, give
  `store/mod.rs` two `readers.read(...)` methods and switch these two over.
- 2026-09-21 (contracts:foundation) — `router/translate.rs` gained the three variants in its two
  exhaustive lists. That file is not in this task's ownership list, but the three one-line arms are
  what makes the workspace compile, and they say the same thing the `MoveCard` arm beside them does.
- 2026-09-21 (d:core-engine) — `brief` opens a skill action with its own `/{name} {args}` line.
  `CardRunRequest` (contracts §3.3) carries no skill field, so the invocation has nowhere else to
  travel, and that is exactly why `validate_automation` refuses a skill action on Codex: only
  Claude reads one out of a first message. Neither contracts §2 nor the phase plan lists the line;
  without it a skill column would start a child that never runs the skill.
- 2026-09-21 (d:core-engine) — `RunStarted` drops an unchosen provider with its separator, the way
  the sentence already drops an unchosen model or effort. What the column and the card left open is
  the daemon's configured default (`start_for_card`, §3.5), which a pure engine cannot know and must
  not guess into a sentence a person reads as fact: a card with no preference reads `Run started`.
- 2026-09-21 (d:core-engine) — an automatic move swaps the `Moved` entry `ops::move_card` appends
  for its own `AutoMoved` one, because `Moved` is reserved for a human's or the CLI's move and
  `ops::attention` reads a `Moved` newer than a run's end as "a person has seen this". `move_card`
  is owned elsewhere this round; if P3-T04 needs the same swap for the `on_success` move, an
  `ops::cards` helper taking the kind and the message is the place to put it once.
- 2026-09-21 (d:core-engine) — rule 2 also skips a dependant that is in `in_flight`, which contracts
  §2 does not list: a reserved card is a card whose run exists in every way but the row, and
  advancing it would strand that run in a column the card had left.
- 2026-09-21 (d:triggers-cards) — `Automation::in_flight` is private to `boards::automation`, and
  Rust field privacy is by module, so a sibling trigger site in `boards/cards.rs` cannot reach it.
  The trigger sites therefore go through `d:boards-automation`'s `pub(super)
  Boards::evaluate_with_reservation(doc, seeds, now)`, which holds the real set for the length of
  the walk. Resolved in the same round: no trigger site evaluates over a reservation of its own.
- 2026-09-21 (d:triggers-cards) — the working refusals read the card's own newest run row rather
  than `DelegationService::live_for_board`, for the same privacy reason and because the rows are
  what `LiveIndex::from_runs` and `resume_automation` already treat as truth. A row left live by a
  crash therefore refuses a move until the boot sweep adopts or closes it, which is the safe half.
- 2026-09-21 (d:triggers-cards) — `delete_card` seeds the cards it freed, as the plan says, but the
  engine's rule 2 advances the *dependants of a visited card*, so a card whose only blocker was the
  deleted one is never itself considered for `advance_when_unblocked` — the blocker it would be
  seeded from no longer exists. Closing it needs the engine to apply rule 2 to a seed's own state;
  contracts §2 ("seeds are … every card whose blocker changed satisfaction") reads as if it should.
- 2026-09-21 (d:triggers-cards) — `create_card` and `update_card` now call `validate_links` beside
  `validate_parent`, which contracts §1.7 assigns to "the daemon" and no phase-3 task claimed. Both
  call sites live in `boards/cards.rs`; without them a self-block, a cycle or a blocker on another
  board persists and every later edit of that card fails validation.
- 2026-09-21 (d:boards-automation) — the reservation is shared through
  `Boards::evaluate_with_reservation(doc, seeds, now)` rather than the
  `pub(super) fn in_flight()` accessor `d:triggers-cards` asked for: handing a `Mutex` out of the
  module would let any caller hold it across a gate, which is the one lock order that deadlocks
  this service. `boards/cards.rs::evaluate` closes its over-start gap by delegating to it.
- 2026-09-21 (d:boards-automation) — a run whose start was refused writes a `RunEnded` activity
  entry beside its `CardRun`, which contracts §3.5 does not list. The evaluation has already
  written `RunStarted` by then, and an opened entry nothing closes reads on the card as a run
  still working.
- 2026-09-21 (d:boards-automation) — when neither the card nor the column names a provider the
  daemon runs Claude (`DEFAULT_PROVIDER`). `CardRun.provider` is not optional, so a default has to
  be picked somewhere, and Claude is the one provider every action kind can run: `validate_automation`
  refuses a skill action on Codex outright. The engine still drops the unnamed provider from the
  `Run started` sentence, so nothing claims the card asked for it.
- 2026-09-21 (d:boards-automation) — `start_run` refuses a live run with `{KEY} is working; cancel
  the run first`, the sentence contracts §3.5 gives the delete and archive refusals. The table
  fixes the *kind* (`Conflict`) for this row but no words, and inventing a second sentence for the
  same situation is how two surfaces start disagreeing.
- 2026-09-21 (d:boards-automation) — `wait_run` returns as soon as the delegation is terminal,
  which can be a moment before `on_run_delivered` has written the outcome onto the card, so a CLI
  reading `runs.last()` may still see the run live. That is what the plan specifies; if it shows
  up in phase 5, the fix is for `wait_run` to wait on the board write rather than on the
  delegation.
- 2026-09-21 (d:lifecycle) — `update`'s three `automation` refusals run only when the patch *asks*
  for automation (a column gained or changed an automation block, or `max_live_runs` was set or
  raised), not whenever the resulting board carries any. Holding every patch to them would strand
  a board whose worktree a host adopted afterwards: it could no longer be renamed, and — worse —
  its automation could no longer be taken off. The plan's "the same three run again at entry" is
  what catches the adoption, and `Boards::require_automatable` is `pub(crate)` so the entry path
  can call the one implementation.
- 2026-09-21 (d:lifecycle) — the entry-side refusal in `boards/automation.rs::card_request` raises
  `DaemonError::Validation("automation is available on worktree boards only")` while `update`
  raises `BoardError::Invalid { field: "automation", .. }`, which renders as `invalid automation:
  automation is available on worktree boards only`. Contracts §1.7 makes these `Invalid` with the
  field `automation`, so the two sentences differ by that prefix today. One of them should change
  before phase 6 pattern-matches on either.
- 2026-09-21 (d:lifecycle) — a column is refused removal while a *run row* still names it, not
  merely while cards stand in it: a card can be moved on while its run is live, and the run is
  then the only thing keeping that column meaningful. The refusal is raised ahead of the existing
  "status is used by card" check so the more specific answer wins.
- 2026-09-21 (d:lifecycle) — `update` evaluates through `Boards::evaluate_with_reservation`, the
  `pub(super)` walk `d:boards-automation` landed, so this path holds the board's one reservation
  like every card trigger site does. Only the *tail* is spelled out again rather than shared with
  `cards::commit`: `update` saves with `Updated`, not `CardChanged`.
- 2026-09-21 (d:resume) — boot recovery seeds only the cards the board *owes* a run (a `pending_run`,
  or a start announced and never recorded), not every card as P3-T05 words it. The engine's rule 1
  cannot tell a card that entered an action column from one that has stood in it since before the
  restart, so seeding every card would start a run for every card sitting in an action column —
  which is the very thing the same sentence forbids, and on a board whose runs have all finished it
  would re-run all of them on every daemon restart.
- 2026-09-21 (d:resume) — a run row whose delegation is terminal is classified by the delegation's
  *delivery state*, which §3.5 does not mention: `Pending`/`Delivered` is still owed by the outbox,
  so the row is adopted and the worker's drain closes it; `Recorded`/`Consumed`/`Undeliverable`
  means no delivery will ever come again, so the sweep records it by calling `on_run_delivered`
  itself (idempotent by delegation id) rather than inventing a second outcome path. Only a
  delegation the store no longer holds is closed `Incomplete` with the lost-record sentence. That
  is also why the maintenance task does not wait for the worker's first drain — the worker exposes
  no signal for one, and recovery no longer depends on the order.
- 2026-09-21 (d:resume) — a live delegation whose card carries no row for it is adopted with a row
  stamped from the delegation's own `created`. That is the crash window between `run_for_card`
  answering and `record_run` writing; without it the card looks idle while its child still edits
  the worktree, and the next evaluation would start a second run beside the first.
- 2026-09-21 (d:resume) — a walk that only re-queues a card already parked for the column it stands
  in does not save. `Plan::queued` alone is not evidence of a change: it is pushed on every walk
  that reaches a parked card, and saving on it would rewrite and broadcast a board document on
  every terminal delegation the daemon sees while any card waits.
- 2026-09-21 (i:daemon-e2e) — the suite boots `HarnessEnv::rooted` + `apply_preset` +
  `Daemon::start` rather than `IsolatedDaemon::start`, which the plan names. `IsolatedDaemon::start`
  lays the environment out *and* spawns the daemon in one call, and a preset has to seed the home
  before any daemon owns it. Everything else is `IsolatedDaemon`'s own code — same `HarnessEnv`,
  same `Daemon`, same temporary root in the same drop order — and `Daemon::start` is also what the
  restart test needs to put a second `fleetd` on one home.
- 2026-09-21 (i:daemon-e2e) — the diamond needs one human move to start. The plan's wording ("all
  moved to Ready") would start nothing: rule 2 advances the *dependants* of a visited card, so a
  card with no blockers standing in the routing column is never released by anything, and rule 1
  only fires on the column a card is *in*. The test therefore moves A into the action column, which
  is the gesture a person makes, and the other three are released by the engine alone. This is the
  same gap `d:triggers-cards` recorded for `delete_card`, seen from the other end.
- 2026-09-21 (i:daemon-e2e) — the "never two at once" assertion polls the board's own run rows
  instead of watching `DelegationChanged`, which the plan asks for. `fleet-client`'s `hello_client`
  names only `AGENT_CAPABILITIES`, so no client names `board.automation` in *its* Hello yet and the
  daemon's per-peer filter (§15.7) hides every card-called event and listing row from it. The
  persisted rows are what a peer can actually see today, and the test also proves the invariant
  without sampling, from the rows' own `started_at`/`ended_at` intervals.
- 2026-09-21 (i:daemon-e2e) — the hosted-worktree refusal is provoked by writing the `host` onto the
  worktree record *under the running daemon*, not before a boot. Startup recovery
  (`services/worktrees/recovery.rs::migrate_legacy_remote_records`) moves every persisted remote
  worktree into the legacy archive and drops it from state, so a hosted record written before a
  boot is gone by the time anything could refuse it; and the mirror, which is the production source
  of that answer, needs a second linked daemon. `StateStore::load` reads the file rather than a
  memo, so the very next refusal check sees the edit.
- 2026-09-21 (i:daemon-e2e) — `CardRunStart`'s proto doc comment said "now, whatever the throttle
  says", which `Boards::start_run` does not do: it seeds one evaluation, and an evaluation parks a
  card the board's ceiling has no slot for. The behaviour is right — the runs of one board share one
  checkout, and no request may make that untrue — so the comment was corrected rather than the code,
  and `docs/BOARD.md` §5 and §11.7 say the same thing.
- 2026-09-21 (integration) — the `live_runs` read join landed as `Boards::live_runs(board)` in
  `boards/automation.rs`, with a free `card_live_run` mapping one `Delegation` to a `LiveRun` and
  dropping a caller that is not a card. `Boards::get` calls it after dropping the card-index write
  guard, so no lock is held across the delegation read. `Boards::list` still passes `&[]` to
  `summarize`: the run rows on the cards already carry `working_count`, and a listing that joined
  one delegation query per board would pay N reads for a number it already has.
- 2026-09-21 (integration) — `wait_run` falls back to `DelegationService::live_for_card` when the
  card's own newest row names no live run. That is the crash window `resume` documents — a run
  started under a daemon that died before `record_run` landed — and a wait that answered "nothing
  is running" about a live child would be wrong rather than merely stale.
- 2026-09-21 (integration) — `prepare_run` is now `async` and runs `require_automatable` before
  building the request, which is contracts §1.7's "the same three run again at entry": a worktree
  adopted by a host after its column was written now refuses the *run*, with the same
  `invalid automation: …` sentence `update` raises. `card_request`'s own worktree guard became
  unreachable and was respelt as `BoardError::Invalid` so the two paths cannot drift; the two unit
  tests that pinned `validation failed: …` were updated with it. The *rendered* detail on a
  failed-to-start row is now `validation failed: invalid automation: …`: `BoardError::Invalid`
  converts into `DaemonError::Validation`, so the field name rides inside the sentence exactly as
  it does for every other board refusal a card records.
- 2026-09-21 (i:daemon-e2e) — `fleet-client` is now a dev-dependency of `fleet-daemon`. The
  end-to-end suites talk to their private `fleetd` through the typed client every other peer uses
  instead of a second hand-rolled socket helper; no cycle, and `fleet-harness` already depends on it.

## Follow-ups
- ~~**Phase 6/7, before any surface renders a run mark.** `fleet-client`'s `hello_client`
  names `AGENT_CAPABILITIES` only~~ — done 2026-09-21 at integration: `hello_client`
  (`crates/fleet-client/src/connection.rs`) now chains
  `fleet_proto::response::BOARD_AUTOMATION_CAPABILITY`, so every client sees card-called
  `DelegationChanged` events and card-called listing rows. `tests/boards_automation.rs` still
  asserts the throttle from the board's run rows, which is the stronger assertion anyway.
- **The hosted-worktree refusal has no honest end-to-end reproduction on one daemon.** In
  production the answer comes from the *mirror*, which needs a second linked daemon, and a hosted
  record written into local state before a boot is migrated away by startup recovery. The sentence
  is covered deterministically by `services/boards/tests/refusals.rs` and, at the wire, by
  `tests/boards_automation.rs` writing the host under the running daemon. A two-daemon version
  belongs with `tests/remote_boards.rs` if ADR 0021's routing ever changes.
- **`Boards::get` now takes one delegation read per board refresh, on the *writer* pool.**
  `DelegationService::live_for_board` goes through `delegation_write` because the reader pool has
  no typed card seam (contracts:daemon's own note), and `get` is the app's refresh path after
  every `BoardChanged`. Nothing deadlocks — every path takes the board gate before the delegation
  writer, and the delivery hook is awaited with no writer held — but a busy outbox can now add
  latency to a board read. The remedy is named: two `self.inner.readers.read(...)` methods in
  `services/agents/store/mod.rs` for `live_for_board`/`live_for_card`.
- **`Boards::list` joins nothing.** `summarize` is passed `&[]`, so `working_count` comes from the
  cards' own run rows. That is right today — the rows are this daemon's record of what is running
  — but a listing of N boards would otherwise pay N delegation reads for a number it already has.
  If a summary ever needs a fact only the delegation store holds, the join has to be batched.
- **`services/boards/automation.rs` is 996 production lines plus a ~520-line inline `mod tests`,
  and `lifecycle.rs` is 983.** Both are over `rust-workspace-architecture`'s ~900-line line. The
  seams are already visible in `automation.rs`: the run verbs, the delivery hook
  (`on_run_delivered` + `report_excerpt` + `files_section`) and the request builders are three
  subjects that barely share a helper, and the inline tests should move to
  `boards/automation/tests.rs` the way `resume.rs` already has its own module. Deliberately not
  split at integration — four owners wrote into these files in one round and a move now would
  bury their diffs — but it is the first thing the next change to either should do.
- **`fleet-daemon`'s `tests/` is now 29 binaries.** `rust-gpui-testing` asks for consolidation
  under one `[[test]]` stanza; `boards_automation.rs` is a fair first member of a future
  `tests/integration/boards.rs` alongside `boards_service.rs` and `boards_jira.rs`.
