# Board workflows, phase 3: the automation engine — Plan
> Tracker: ./board-workflows-2026-09-20-phase-3-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you
  commit.

## Summary

The board starts driving work. A pure engine in `fleet-core::board::automation` takes a document
already loaded under the board gate, walks breadth-first from the cards whose column changed, and
returns a `Plan { starts, queued, moved }` the daemon applies after the gate is released. Five
trigger sites in `Boards` call it: `move_card`, `update_card` with a status or `archived` change,
`create_card` with a status, `delete_card`, and `update` when a category changes or a column loses
its action. `start_for_card` resolves prefs, assembles the brief and the card footer and calls phase
2's `run_for_card`; `on_run_delivered` writes the terminal facts, the report excerpt and the
`on_success` move, then re-evaluates. `resume_automation` adopts live delegations after a restart, a
bus subscriber hands a freed slot to the next pending card, the three run requests and
`BoardView.live_runs` go on the wire, and `board.automation` is advertised.

## Sizing call

**Phased, phase 3 of 9.** See ./board-workflows-2026-09-20-roadmap.md. The largest daemon phase: a
pure module, a service submodule, five trigger sites, two apply paths, a boot sweep, a subscriber,
three wire verbs, a capability. One phase, because the throttle is only correct when the
reservation, the triggers and the delivery hook are reviewed together; one pull request, because a
half-wired engine starts runs nothing closes. The pure engine and its table tests come first.

## Repository context

- Gate `services/boards.rs:99-105` (`lock_owned()`, map `:40`), `now()` `:107`, `changed()`
  `:111-117`. **Non-reentrant**: `Boards` never calls its own `pub` verbs. Only `save` emits
  (`boards/documents.rs:104-112`); `persist` `:86-102` is silent; `card_document` `:134`.
- Verbs `boards/cards.rs:5` create, `:20` update, `:91` move, `:108` delete;
  `boards/lifecycle.rs:436` `update`, `:47-57` `get`, `:145-213` `summaries`, `:696-715`
  `worktree_context`. `Worktree.host` `fleet-core/src/model.rs:143`; `backend.is_local()`
  `cards.rs:171`.
- Delegation `delegation/mod.rs:60`; `run.rs:88` (validation `:98-191`), `:61-83`
  `resolve_fleet_program`, `:311` `path_prepend`; `queries.rs:341` `get`, `:352` `wait`;
  `cancel.rs:19`; `limits.rs:9` `MAX_LIVE_DELEGATIONS = 8`.
- Boot `composition.rs:89-90` delegation install, `:91-102` `Boards::new`, `:103` cascade;
  `maintenance.rs:88-107` task list, `:106` outbox worker, `:68-73` abort on drop. Subscriber
  precedent `server/listener.rs:398-425`; bus `server/broadcast.rs:29`.
- Wire `request.rs:496-503` `MoveCard`, `response.rs:59` `BOARD_WORKTREE_CAPABILITY`,
  `router/classify.rs:108-124`, `dispatch.rs:230-238`, advertisement `server/connection.rs:924-935`
  + mirror `:1029-1036`; client `api/boards.rs:151`, `:101-107`, `connection.rs:1157-1172`,
  `:679-748` (`AGENT_HARNESS_TIMEOUT` `:46`).
- Tests `services/boards/tests.rs:11` `fixture()`; `tests/boards_service.rs:36-50`, `:166`
  `reopened`. `FakeProvider` (`agents/manager/tests/mod.rs:239`) and `MockPeer`
  (`agents/harness/mockpeer.rs`) are `#[cfg(test)] pub(crate)`, unreachable from `tests/`.
- Scripted provider `fleet-harness/src/env.rs:519-598` `IsolatedDaemon` (`start` `:531`,
  `environment()` `:562`, `connect()` `:583`), `install_fake` `env.rs:172`, `agent/launcher.rs:57`
  `launcher_script` (child transcript when `FLEET_DELEGATION` is set `:80-84`, binary lookup
  `:93-110`), `fixture/tools.rs:144-170`, `transcripts/subagent-child.json`,
  `docs/TESTING-HARNESS.md` §5.
- Skills: `rust-async-background-work`, `rust-ipc-protocol`, `rust-workspace-architecture`,
  `rust-gpui-testing`, then `zed-quality-review`. Docs: `docs/BOARD.md` §4 (`:455-610`), §5
  (`:611`), §6 (`:647`), §11; `docs/NATIVE-AGENTS.md` §15.7.

## Assumptions

- Phase 1 landed contracts §1; phase 2 landed §3.1–§3.3 including `RunDeliveryHook`, `run_for_card`,
  `live_for_board`, `live_for_card`, `set_run_delivery_hook`.
- `CardRunRequest` has no `fleet_path`: the daemon passes `None` and the daemon-sibling rule finds
  `fleet` beside `fleetd` (contracts §3.3).
- `Automation` is `Option`: with no agents database, boards still work and every run verb answers
  §3.5's `Unsupported` sentence.
- The reservation is memory only; after a restart in-flight degrades to `pending_run`, which
  `resume_automation` adopts or restarts.

## Out of scope

- `changed_since`, `files_changed` and the files brief section — phase 4 (phase 3 writes
  `files_changed = 0`).
- CLI verbs (phase 5); app, kit, keys and dialogs (phases 6 to 9); rework, spend caps and per-column
  throttles (refused in the design).

## Affected areas

- New `fleet-core/src/board/automation.rs` + `automation/tests.rs`; `board.rs`.
- New `fleet-daemon/src/services/boards/automation.rs`; `services/boards.rs`, `boards/{cards.rs,
  lifecycle.rs, sync.rs, tests.rs}`, `services/{composition.rs, dispatch.rs, maintenance.rs,
  router/classify.rs}`, `server/connection.rs`.
- `fleet-proto/src/{request.rs, response.rs}` + `tests/compatibility.rs`;
  `fleet-client/src/{api/boards.rs, connection.rs}`.
- New `fleet-daemon/tests/boards_automation.rs`; `fleet-daemon/Cargo.toml`; `docs/BOARD.md`,
  `docs/NATIVE-AGENTS.md`.

## Tasks

### P3-T01 — The pure engine and its table tests
- **Intent:** Every decision the throttle and the cascade make, with no daemon, provider or clock.
- **Touches:** `fleet-core/src/board/automation.rs`, `board/automation/tests.rs`, `board.rs`.
- **Steps:**
  - `LiveIndex::{from_runs, contains, len}`, `StartRun`, `Plan`, `ResolvedPrefs`, and contracts §2's
    `re_evaluate`, `next_pending`, `resolve_prefs`, `brief` with the signatures verbatim.
  - `re_evaluate`: breadth-first from `seeds` over `ops::query::blocks` with a `seen` set. (1)
    Column has `on_enter`, no live run, not in `in_flight` → if `live.len() + in_flight.len() <
    settings.max_live_runs()` push `StartRun`, insert into `in_flight`, push `RunStarted` (`Run
    started · {provider} · {model} · {effort}`); else set `pending_run = { status_id, since: now }`
    and write nothing. (2) Each dependant whose column has `advance_when_unblocked`, whose blockers
    are all `is_satisfied`, with neither a live run nor a `pending_run` → `ops::move_card` in
    memory, push `AutoMoved` (`Moved to {column name}: unblocked by {KEY} reaching {column name}`),
    enqueue.
  - `next_pending`: later column first by index in `board.statuses`, ties by oldest `since`.
    `resolve_prefs`: card → column action → `None`; `mode` from the column else `FullAccess`.
    `brief`: templated instructions, `# {KEY} — {title}`, the description, `## Previous run reports`
    newest first — no footer, no files section.
  - Table tests: the diamond visits each card once; two pending cards resolve later-column-first
    then oldest-`since`; a hand-built cycle terminates through `seen`; re-entering a column with a
    live run adds no second `StartRun`; a column that lost its action starts nothing; a full
    `LiveIndex` queues.
- **Verification:** `cargo test -p fleet-core board::automation`, `make lint`.
- **Done when:** Every rule has a named test and the module imports nothing outside
  `fleet_core::board`.

### P3-T02 — `Automation`, `Boards::new` and the composition wiring
- **Intent:** The reservation, the delegation handle and the delivery hook, with no way to re-enter
  the gate.
- **Touches:** `boards/automation.rs`, `services/boards.rs`, `composition.rs`, `maintenance.rs`,
  `tests/boards_service.rs`.
- **Steps:**
  - `pub(crate) struct Automation { delegations, checkpoints: Arc<Checkpoints>, in_flight:
    Mutex<BTreeSet<CardId>>, pending_boards: Mutex<BTreeSet<BoardId>> }` as §3.5; `checkpoints` is
    stored unused until phase 4, with a doc comment saying so.
  - `Boards::new(/* existing */, automation: Option<Automation>)`; `composition.rs:91-102` passes
    `Some` iff `delegation::install` returned `Some`; update `boards_service.rs:166`. Then
    `delegations.set_run_delivery_hook(Arc::downgrade(&boards))` beside
    `worktrees.set_cascade(boards)` (`:103`) — a `Weak`, so the hook never keeps the service alive.
  - Module doc carries the gate rule: only request handlers, `resume_automation`,
    `on_slot_released` and `on_run_delivered` take the gate, and every start happens after the
    guard is dropped.
  - `maintenance.rs`: after `run_delegation_outbox` (`:106`), spawn `run_board_automation(services,
    shutdown)` — await the worker's first drain, call `resume_automation` once, then subscribe and
    call `on_slot_released` on every terminal `DelegationChanged` while `pending_boards` is
    non-empty; `Lagged` re-checks every memoised board.
- **Verification:** `cargo test -p fleet-daemon services::boards`, `cargo check --workspace
  --all-targets`, `make lint`.
- **Done when:** A daemon with no agents database still builds `Boards` and `composition.rs:322`
  passes.

### P3-T03 — The five trigger sites and the daemon refusals
- **Intent:** Every path that can change a satisfaction re-evaluates under the gate it already
  holds.
- **Touches:** `boards/cards.rs`, `boards/lifecycle.rs`, `boards/automation.rs`, `boards/sync.rs`.
- **Steps:**
  - Shared shape: gate → reload → apply in memory → seed → `re_evaluate` → save **once** → drop the
    guard → apply `plan.starts` through `start_for_card`.
  - `move_card(card, status, index, cancel_run)`: `Conflict` `{KEY} is working; pass --cancel-run to
    move it` when a run is live and the flag is false; with the flag, cancel first. A `pending_run`
    is cleared silently — the `Moved` entry already says what the user did.
  - `update_card`: seed on a `status_id` change and on `archived` either way; archiving a card with
    a live run is `Conflict` `{KEY} is working; cancel the run first`. `create_card` with a status
    seeds the new card.
  - `delete_card`: the same working refusal; otherwise drop the card from every dependant's
    `blocked_by` in the same save with `Unblocked: {KEY} was deleted`, and seed the former
    dependants.
  - `update`: seed every card whose status category changed; a column that lost `on_enter` clears
    matching `pending_run`s with `Run canceled: {column name} no longer runs an action`; removing a
    column with live runs is `column has {n} live runs; cancel them first`; `max_live_runs` outside
    `1..=MAX_LIVE_RUNS_PER_BOARD` is `must be between 1 and 8`; automation is refused with §1.7's
    `automation is available on worktree boards only`, `automation is available on local boards
    only` (`!backend.is_local()`), `automation is unavailable on a worktree owned by host {host}`
    (`worktree_context(..).0.host`). The same three run again at entry, because a worktree can be
    adopted by a host later.
  - `boards/sync.rs` reconcile never calls `re_evaluate`; one comment line says a pull is not a user
    action.
- **Verification:** `cargo test -p fleet-daemon services::boards`, `make lint`.
- **Done when:** Each refusal has a test needing no provider and no trigger site calls a `pub` verb
  of `Boards`.

### P3-T04 — `start_for_card` and `on_run_delivered`
- **Intent:** A `StartRun` becomes a delegation; a finished delegation becomes card facts. Both
  outside anyone's gate.
- **Touches:** `boards/automation.rs`; reads `delegation/footer.rs`.
- **Steps:**
  - `start_for_card(board, card)`: prefs card → column → the provider config default for the model
    (the delegation service's `Inner.config`); mode from the column, `full_access` default; brief +
    `CARD_FOOTER_TEMPLATE` (§3.3) with `{id}`, `{key}`, `{board}`, `{expectation}`, `{fleet}`
    substituted — an empty expectation prints `The card expects: (the column names no expectation)`;
    title `↳ {KEY} — {title cut at 48}`; `env` values through `render_template`.
  - `Ok` → re-acquire the gate, push the `CardRun` (cap `MAX_RUNS_PER_CARD`, drop oldest), clear
    `pending_run`, remove from `in_flight`, save. `Err` → re-acquire, push `CardRun { id: DelegationId::new(), thread_id: None, outcome:
    Some(Failed), ended_at: Some(now), detail: Some(sentence), .. }` (contracts §3.5), clear, remove,
    save; phase 6's `apply_board_view` turns that `CardChanged` into a sticky error.
  - `impl RunDeliveryHook for Boards::on_run_delivered`: gate; a run already terminal returns `Ok`
    writing nothing; else write `ended_at` and `outcome` — Succeeded→`Succeeded`; Failed with
    `status_payload == "reported blocked"`→`NeedsYou`; Failed→`Failed`; Incomplete→`Incomplete`;
    Cancelled→`Cancelled` — `detail = status_payload`, `files_changed = 0`, `cost_usd`/`tokens` from
    `delegations.get(id)`.
  - Report comment: the text capped at `REPORT_EXCERPT_CAP_BYTES` on a char boundary with `…` and
    the trailing line `(report elided; the full report is in the run's thread)`, `run_id` set, its
    id stored on the run; drop the oldest report comment past `MAX_REPORT_COMMENTS_PER_CARD`,
    clearing that run's `report_comment_id`.
  - `Succeeded` with `on_success` on the *current* board → `ops::move_card` with `Moved to {column
    name}: run succeeded`. Then `re_evaluate` seeded with this card, save once, drop, apply starts.
    `Err` leaves the `Deliver` row open for the next drain.
- **Verification:** `cargo test -p fleet-daemon services::boards`, `make lint`.
- **Done when:** Both paths re-acquire the gate rather than holding one, and a second delivery of
  the same id writes nothing.

### P3-T05 — Run verbs, `resume_automation`, the slot subscriber, the read joins
- **Intent:** The three verbs, boot recovery, the freed slot, and what a reader sees.
- **Touches:** `boards/automation.rs`, `boards/lifecycle.rs`, `maintenance.rs`,
  `fleet-core/src/board/ops/query.rs`.
- **Steps:**
  - `start_run`: `Validation` `{column name} has no action` outside an action column, `Conflict`
    when a run is live, else seed and start — a re-run after any outcome is allowed. `cancel_run`:
    `NotFound` `{KEY} has no live run`, else `delegations.cancel(id)`; `on_run_delivered` writes the
    `Cancelled` facts. `wait_run`: a terminal newest run returns at once; a live one waits on
    `delegations.wait(id, timeout_ms, None)` then re-reads; expiry returns the card as it is.
  - `resume_automation()`: once, after the worker's first drain. Per worktree board under its gate —
    a card whose newest run is not terminal reads its delegation and adopts it, or closes it
    `Incomplete` with a `detail` saying the daemon lost the record; a card with `pending_run` first
    adopts a live delegation whose caller is that card (`live_for_card`) before it would start a
    second; then **one** `re_evaluate` seeded with every card. Nothing fires for cards merely
    sitting in an action column.
  - `on_slot_released()`: per board in `pending_boards`, under its gate, take `next_pending` and
    re-evaluate; drop the board from the memo when it holds no `pending_run`. The memo is written
    wherever a `pending_run` is set or cleared.
  - `BoardView.live_runs` is joined on read in `get` (`lifecycle.rs:47-57`) and both `ensure*` paths
    (confirm with `grep -n "fn ensure" crates/fleet-daemon/src/services/boards/lifecycle.rs`) from
    `live_for_board`, never persisted.
  - `ops::summarize` gains `live: &[LiveRun]` and `now: &str`, filling `working_count` (live +
    pending) and `attention_count` (`ops::query::attention`); update `lifecycle.rs:32`, `:183`,
    `ops/query.rs:54-81`.
- **Verification:** `cargo test -p fleet-daemon services::boards`, `cargo test -p fleet-core board`,
  `make lint`.
- **Done when:** `summaries` still answers from its memo when nothing changed, and the subscriber
  costs nothing while `pending_boards` is empty.

### P3-T06 — The wire, the client and the capability
- **Intent:** Three requests, one flag, one capability, byte-exact.
- **Touches:** `fleet-proto/src/{request.rs, response.rs}` + `tests/compatibility.rs`,
  `services/{dispatch.rs, router/classify.rs}`, `server/connection.rs`,
  `fleet-client/src/{api/boards.rs, connection.rs}`.
- **Steps:**
  - `MoveCard` gains `#[serde(default, skip_serializing_if = "std::ops::Not::not")] cancel_run:
    bool`; add `CardRunStart { card_id }`, `CardRunCancel { card_id }`, `CardRunWait { card_id,
    timeout_ms }`, each answering `ResponseBody::Card(Card)`; all three classify `Target::Local`;
    three dispatch arms reach `Boards::{start_run, cancel_run, wait_run}` and `cancel_run` is
    forwarded on `MoveCard`.
  - Client `card_run_start`, `card_run_cancel`, `card_run_wait(CardId, u64)`, `move_card(..,
    cancel_run: bool)`. `required_capability` maps the three to `BOARD_AUTOMATION_CAPABILITY` with
    ``this daemon does not support board automation; run `fleet daemon restart` ``;
    `request_timeout`: `CardRunStart => AGENT_HARNESS_TIMEOUT`, `CardRunCancel => None`,
    `CardRunWait { timeout_ms } => timeout_ms + 15 s`.
  - Advertise `BOARD_AUTOMATION_CAPABILITY` at `server/connection.rs:924-935`; update the mirror
    `:1029-1036` and `machines/link.rs:604` if it enumerates.
  - Goldens: the three requests, `MoveCard { cancel_run: true }`, a `BoardView` with one `live_runs`
    entry, a `Card` with every new field; the legacy `Card` fixture (`compatibility.rs:371-400`) is
    not edited.
- **Verification:** `cargo test -p fleet-proto`, `cargo test -p fleet-client`, `cargo test -p
  fleet-daemon router`, `make lint`.
- **Done when:** A `Hello` lists `board.automation` and every golden is byte exact.

### P3-T07 — The end-to-end suite and the docs
- **Intent:** Prove the diamond, the restart and the refusals against a real `fleetd` and a scripted
  provider.
- **Touches:** `fleet-daemon/tests/boards_automation.rs`, `fleet-daemon/Cargo.toml`,
  `services/boards/tests.rs`, `docs/BOARD.md`, `docs/NATIVE-AGENTS.md`.
- **Steps:**
  - Add `fleet-harness` to `fleet-daemon`'s `[dev-dependencies]` — no cycle: it depends on
    `fleet-client`, `fleet-core`, `fleet-drive`, `fleet-proto` only.
  - Boot: `IsolatedDaemon::start("boards-automation")` (`env.rs:531`) spawns a private `fleetd`
    synchronously with its own `FLEET_HOME`, child `HOME` and a `PATH` whose first entry is the
    run's `bin/`; **before** `connect()` (`:583`) awaits readiness, write `codex-transcript.json`
    and, beside it, `subagent-child.json` — a copy of `transcripts/subagent-child.json` whose
    `{"type":"shell"}` step writes a report and runs `fleet subagent complete --result-file` — then
    `daemon.environment().install_fake("codex",
    &fleet_harness::agent::launcher_script(Provider::Codex, &transcript)?)` (`env.rs:172`,
    `launcher.rs:57`). The launcher plays the child transcript because the daemon sets
    `FLEET_DELEGATION` (`launcher.rs:80-84`), and the child's `fleet` resolves through
    `path_prepend` (`run.rs:61-83`, `:311`). `FLEET_HARNESS_BIN` comes from `make test`
    (`Makefile:62`); a bare `cargo test` falls back to `launcher.rs:93-110`.
  - `a_four_card_diamond_never_runs_two_delegations_at_once`: preset columns, four cards linked A→B,
    A→C, B→D, C→D, `max_live_runs = 1`, all moved to Ready; watch `DelegationChanged` and assert the
    live count never exceeds one, every card ends in Done, and each card's `runs` is exactly what
    its columns produced, in order.
  - `a_restart_mid_run_adopts_the_live_delegation`: shut the daemon down mid-run (`daemon_mut`),
    start a second `fleetd` on the same home, assert one run and no second delegation.
  - `automation_is_refused_on_a_context_board_a_jira_board_and_a_hosted_worktree`, §1.7's sentences
    verbatim. Unit tests in `services/boards/tests.rs` for refusals needing no provider: no action
    on the column, no live run to cancel, `max_live_runs` out of range, the no-delegation-service
    `Unsupported` sentence.
  - Docs: `docs/BOARD.md` §4 gains the trigger list, the plan-outside-the-gate rule and the refusal
    table; §5 the three requests and the capability; §6 the client methods; §11 the engine half —
    entry loop, reservation, throttle order, outcome table, restart rules. `docs/NATIVE-AGENTS.md`
    §15.7 gains the sentence that a card caller's `Deliver` row is marked done only after
    `on_run_delivered` returns `Ok`.
- **Verification:** `cargo test -p fleet-daemon --test boards_automation`, `make test`, `make
  restart`, `make lint`.
- **Done when:** All three end-to-end tests pass twice on a cold target directory and the documents
  agree with the code.

## Verification

```sh
make lint
cargo test -p fleet-core board::automation
cargo test -p fleet-daemon services::boards
cargo test -p fleet-daemon --test boards_automation
cargo test -p fleet-proto && cargo test -p fleet-client
make test && make restart
```

## Definition of done

- [ ] Every task above is `[x]` in the tracker.
- [ ] `make lint` and `cargo check --workspace --all-targets` are clean; `make test` passes.
- [ ] `board.automation` is advertised and gates the three requests on the client.
- [ ] No `Boards` method calls a `pub` verb of `Boards`; the reviewer checked the call graph.
- [ ] `docs/BOARD.md` §4, §5, §6, §11 and `docs/NATIVE-AGENTS.md` §15.7 match the code.
- [ ] The tracker reflects reality; follow-ups are captured.

## Risks and rollback

- **Gate re-entry** hangs a board forever. Mitigated by the module-doc rule and a review pass over
  every `self.` call in `boards/automation.rs`.
- **Two claimants for one freed slot.** Both count `in_flight` and serialise on the gate; the loser
  writes `pending_run`. The diamond test proves it.
- **A delivery racing a restart** is a no-op: `on_run_delivered` is idempotent by delegation id and
  the `Deliver` row closes only after the board write commits.
- **Rollback.** Revert the phase PR. Version-2 documents still load, cards keep an unread `runs`
  history, and nothing fires because no trigger site exists.

## Contract resolutions

Resolved in the contracts file on 2026-09-20; the contracts wording wins over any earlier phrasing above.

1. `CardRunRequest` has no `fleet_path`; `run_for_card` takes `resolve_fleet_program`'s daemon-sibling branch (contracts §3.3).
2. The files section is taken at delivery and lives in the delivered report comment; `start_for_card` assembles brief + footer only, and phase 3 writes `files_changed = 0` (contracts §3.4, §3.5).
3. `ops::summarize`'s new arguments are introduced in phase 1 with `&[]` at every caller; phase 3 owns passing the `live_runs` join (contracts §1.2).
