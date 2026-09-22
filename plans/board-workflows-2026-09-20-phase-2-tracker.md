# Board workflows, phase 2: daemon — card-called delegations — Tracker
> Plan: ./board-workflows-2026-09-20-phase-2-plan.md
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
- [x] P2-T01 — `DelegationCaller`, optional caller turn and item, `DeliveryState::Recorded`
  - verified: `cargo test -p fleet-core delegation` (12 passed), `cargo test -p fleet-proto` (the new
    card-caller golden plus every untouched thread golden), `cargo test -p fleet-daemon --lib
    services::agents` (270 passed), `cargo check --workspace --all-targets`, `cargo clippy
    --workspace --all-targets --all-features -- -D warnings`, all clean.
- [x] P2-T02 — Slot 007: the agents store's first table rebuild
  - verified: `cargo test -p fleet-daemon --lib services::agents::store::migrations` (24 passed,
    seven of them new: the three slot-007 tests, the card round-trip, the terminal-run-is-not-live
    read, the repair-sweep predicate and the thread-caller constraint refusal);
    `cargo test -p fleet-daemon --lib services::agents::store` (90 passed);
    `cargo test -p fleet-daemon --lib services::agents` (283 passed, includes the ladder,
    idempotency, index-seek and slot-hash tests); `cargo check -p fleet-daemon --lib --tests` and
    `cargo clippy -p fleet-daemon --all-targets --all-features -- -D warnings` clean.
    `rustfmt --edition 2024` on the three owned files only.
- [x] P2-T03 — `CardRunRequest`, `run_for_card`, the card footer and the child environment
  - verified: `cargo test -p fleet-daemon --lib services::agents::delegation` (107 passed, five of
    them new in `footer`), `cargo clippy -p fleet-daemon --all-targets --all-features -- -D
    warnings` clean. `CardRunRequest` and `CARD_FOOTER_TEMPLATE` (with its line-by-line test)
    already existed from the foundation skeleton and were left alone; this task filled
    `card_footer`, `card_first_message`, `card_child_title` and `run_for_card`, and extended
    `FLEET_OWNED_CHILD_ENV` to four keys.
- [x] P2-T04 — `RunDeliveryHook` and the three decided worker behaviours
  - verified: `cargo test -p fleet-daemon --lib services::agents::delegation` (107 passed),
    `cargo clippy -p fleet-daemon --all-targets --all-features -- -D warnings` clean. The
    hook trait, `set_run_delivery_hook`, `run_delivery_hook` and the two `live_for_*` reads
    landed with the foundation skeleton; this task filled `deliver`'s card arm, the
    `ThrottleKey` and `consume_for`. The repair-sweep predicate (`AND caller_kind = 'thread'`)
    belongs to `store/delegations.rs` — see d:store-slot007's report (P2-T02).
- [x] P2-T05 — The per-peer event and list filter
  - verified: `cargo test -p fleet-daemon --lib server::connection` — the new
    `events::tests::a_peer_without_the_capability_never_sees_a_card_called_event` and
    `tests::a_peer_without_the_capability_never_sees_a_card_caller` pass, 18 passed in all; the
    three `{event_lag_forces_reconnect_and_full_frames,
    a_wedged_remote_cannot_stall_disconnect_cleanup,
    response_write_failure_after_attach_runs_detach_cleanup}` failures are the environmental
    "is not fleetd … run the suite through `make test`" panic, unrelated to this change.
    `cargo check -p fleet-daemon --all-targets` and `cargo clippy -p fleet-daemon --all-targets
    --all-features -- -D warnings` clean.
- [x] P2-T06 — Goldens, `delegation/tests/card.rs` and the documents
  - verified: `cargo test -p fleet-proto --test agent_compatibility` (25 passed; the card-caller
    golden and `legacy.rs` were **already** landed by P2-T01 and needed nothing — re-run and
    re-read to confirm, not re-written); `cargo test -p fleet-daemon --lib
    services::agents::delegation::tests::card` (7 new tests, all passing);
    `cargo test -p fleet-daemon --lib services::agents::delegation` (114 passed, 107 before);
    `cargo check -p fleet-daemon --lib --tests` and `cargo clippy -p fleet-daemon --all-targets
    --all-features -- -D warnings` clean. `rustfmt --edition 2024` on `tests/card.rs` only.
    Docs: `docs/NATIVE-AGENTS.md` §15.7 plus one §15.2 delivery-table row;
    `docs/research/agents-contracts.md` Delegations (caller enum, optional caller turn/item,
    `Recorded`) and the store section (slot 7).

## Notes / decisions log
- 2026-09-21 (contracts:foundation) — `DelegationCaller` gained a `Display` impl the contract does
  not list. A thread caller renders as its thread id, so every `caller = %…` log line in the worker,
  the queries and the repair sweep is byte for byte what it was; a card renders as `board/card`. The
  alternative was editing eight log sites in files phase 2 owns. Its doc comment forbids using it for
  a column value; the two SQL sites read `caller.thread()` explicitly instead.
- 2026-09-21 (contracts:foundation) — the store cannot hold a card caller until slot 007 lands, so
  `insert` and `reserve` refuse one with `DaemonError::Unsupported("card callers are not stored
  yet")` and `decode` reconstructs only `Thread`. Marked `// CONTRACT STUB (d:store-slot007)`.
- 2026-09-21 (contracts:daemon) — the P2 shapes now exist as a compiling skeleton, so T03/T04 are
  body-only work: `CardRunRequest`, `RunDeliveryHook`, `Inner.run_hook`, `set_run_delivery_hook`,
  `run_delivery_hook()`, `live_for_board`/`live_for_card` (service *and* store),
  `set_result_files`, `run_for_card`, and `footer.rs`'s `CARD_FOOTER_TEMPLATE` (real, with a
  line-by-line test) plus `card_footer`/`card_first_message`/`card_child_title`. Every body is
  `// CONTRACT STUB (d:run-for-card | d:store-slot007 | d:files-at-delivery)`. `tests/card.rs`
  exists and is empty, declared from `tests/mod.rs`, for P2-T06.
- 2026-09-21 (contracts:daemon) — the skeleton's unused items carry
  `#[allow(dead_code)] // <who consumes it>`, the idiom `store/mod.rs::delegation_token_hash`
  already uses. Delete the attribute in the commit that adds the caller; do not leave one behind.
- 2026-09-21 (contracts:foundation) — the worker's per-caller throttle now skips a card caller
  rather than keying on it, and `mirror`, `deliver` and `consume_for` close a card-called row
  without writing to a transcript. Marked `// CONTRACT STUB (d:worker-hook)` for P2-T04.
- 2026-09-21 (d:worker-hook) — `consume_for` needed no behaviour change, only its comment: the
  skeleton's `matches!((caller, current.caller.thread()), …)` already refuses to consume a card
  run, because `caller` is a `ThreadId` and a card's `thread()` is `None`. That *is* contracts
  §3.3's "consumes only when the caller is `Thread(t)` and `t == caller`".
- 2026-09-21 (d:worker-hook) — the card arm of `deliver` lives in its own `deliver_to_card`
  rather than as branches inside `deliver`: it shares nothing with the thread path but the
  outbox row, and a reader of either half should not have to step over the other. An `Err` from
  the hook is returned with context and logged once, by the drain's existing "remains open"
  warning, rather than logged here and swallowed.
- 2026-09-21 (d:peer-filter) — the card arm of `event_visible` requires `agent.delegation` **and**
  `board.automation`, where contracts §3.3 spells only the board one. `agent.delegation` is what
  gates the *decode* of the adjacently tagged `Event::DelegationChanged` variant (this module's
  own doc: a capability filter here is a decode-safety rule, not a preference), so granting
  visibility on the board capability alone would hand an undecodable frame to a peer that named
  `board.automation` without `agent.delegation`. Every peer the plan's tests describe behaves
  identically either way.
- 2026-09-21 (d:peer-filter) — the listing filter went into `enqueue_response`
  (`server/connection.rs`), not `write_response`: `write_response` only ever carries the Hello
  handshake frame plus the test-only writes, while `enqueue_response` is the single site both
  dispatch paths (the inline-answered one and the `pending` pool) funnel every other response
  through, with `client` already in scope. It is a pure `hide_card_callers_from_peer` helper so
  the test asserts the rule without a socket.
- 2026-09-21 (d:peer-filter) — nothing in `fleet-client` names `board.automation` in *its* Hello
  (`hello_client` sends `AGENT_CAPABILITIES` only), so today no peer at all sees a card-called
  record through a listing or an event. That is exactly P2-T05's "done when", and it is also the
  switch phases 6/7 must flip before the app can render a run mark — see the integration note.

- 2026-09-21 (d:run-for-card) — the worktree-host rule refuses, but its sentence says "owned by
  another host" instead of naming the host. `Worktrees` exposes only `path()`, whose remote
  refusal is the single place a worktree's owner is known inside the daemon, and it does not
  return the host; naming it needs a one-method accessor on `services/worktrees.rs`, a file this
  task does not own. The exact accessor and the one-line call-site change are in the report's
  integration notes.
- 2026-09-21 (d:run-for-card) — `FLEET_OWNED_CHILD_ENV` is now four keys, so the store drops
  `FLEET_CARD` and `FLEET_BOARD` from `env_json` along with the two secrets. That is right — both
  are derivable from `delegation.caller` — but the manager's resume path re-inserts only
  `FLEET_DELEGATION` and `FLEET_DELEGATION_TOKEN`, so a *resumed* card child would lose its card
  identity and its CLI would stop refusing a self-move. Fix recorded in the report's integration
  notes for the manager owner.
- 2026-09-21 (d:run-for-card) — `card_footer` treats a whitespace-only expectation as empty. The
  contract fixes the parenthetical for an empty `expect`; a column author who typed a space meant
  the same thing, and a blank `The card expects:` line reads as truncated rather than absent.
- 2026-09-21 (d:store-slot007) — the caller columns are written and read through a new
  `EncodedCaller` on `EncodedDelegation`, not inline in `insert`. The constraint the rebuilt
  schema cannot state — `'thread'` fills thread/turn/item, `'card'` fills board/card — is checked
  once on the way in, because a row that broke it would decode as a *different caller* than it was
  written as and no later read could tell. It refuses with `anyhow::bail!`, the form `insert`
  already used for this case, rather than a `DaemonError` variant: nothing branches on it, and the
  writer maps it for the caller.
- 2026-09-21 (d:store-slot007) — `decode` branches on `caller_kind` rather than inferring the kind
  from which columns are null, so a row comes back as the caller it was written as even once a
  third kind exists. An unknown kind is a hard error, not a fallback to `Thread`.
- 2026-09-21 (d:store-slot007) — `reserve` now applies the live-child ceiling only to a thread
  caller. A card caller has no thread to count children under; what bounds it is the board's own
  `max_live_runs` gate (contracts §3.3), and the daemon-wide ceiling below it still applies to
  both. This is the contract's "skips the live-child-limit rule … keeps the daemon-wide limit
  (re-checked in `reserve`)" put where `reserve` can honour it without a second entry point.
- 2026-09-21 (d:store-slot007) — `mark_missing_callers_undeliverable` gained
  `AND caller_kind = 'thread'` (the store half of P2-T04). Without it the sweep's `NOT EXISTS`
  over `threads` matches every card run, whose `caller_thread` is NULL, and the first drain after
  a restart would mark every pending card delivery undeliverable.
  `the_repair_sweep_skips_card_callers_and_still_sweeps_thread_callers` is the tripwire.
- 2026-09-21 (d:store-slot007) — `slot_007_preserves_every_delegation_row_it_rebuilds` compares
  the whole row by column *name* before and after the rebuild, each value rendered with its SQLite
  type, so a copy that transposed two same-typed columns fails rather than passing. That is the
  failure mode rule 4 cannot catch, and the reason the slot's `INSERT … SELECT` names all 29
  columns twice instead of using `SELECT *`.
- 2026-09-21 (d:goldens-card-tests) — **the golden was already there.** P2-T01 landed
  `card_called_delegation()` and `a_card_called_delegation_carries_its_board_and_card_and_no_caller_turn`
  in `crates/fleet-proto/tests/agent_compatibility.rs`, plus the `DeliveryState::Recorded` byte
  fixture, and `legacy.rs` was untouched. Re-read and re-ran both rather than adding a second
  fixture: one card-caller golden is what the plan asks for, and a duplicate would pin the same
  bytes twice.
- 2026-09-21 (d:goldens-card-tests) — `a_card_called_delegation_completes_and_records_through_the_hook`
  inserts its record with a **known token hash** and a real child thread instead of minting it with
  `run_for_card`. The plaintext token `run_for_card` mints reaches only the child's process
  environment, and `worker::Harness` keeps its provider log private, so a test that went through
  `run_for_card` could never call `complete`. The production entry point is still covered end to
  end by `a_card_run_carries_no_caller_turn_or_item`, which calls `run_for_card` for real.
- 2026-09-21 (d:goldens-card-tests) — `the_repair_sweep_ignores_card_callers` installs a hook that
  always refuses, so the card row stays exactly where the sweep left it and the assertion observes
  the sweep rather than the delivery that would otherwise follow it. It asserts the *drain* path,
  as d:store-slot007's integration note asked; the SQL predicate itself stays covered by
  `migrations::tests::the_repair_sweep_skips_card_callers_and_still_sweeps_thread_callers`.
- 2026-09-21 (d:goldens-card-tests) — `the_drain_throttle_keys_card_runs_on_their_board` proves
  both halves in one pass with three rows (two cards on `work`, one on `other`): the first drain
  records one row per board, the second records the card that waited. A per-card key would record
  all three at once and a per-daemon key only one, so the test fails in both wrong directions.

- 2026-09-21 (integration) — `FLEET_OWNED_CHILD_ENV` (four keys, the caller-may-never-set rule)
  and `FLEET_ROTATED_CHILD_ENV` (two keys, the never-persist rule) are now separate constants.
  `encode_env` filters on the rotated pair alone, so a card run's `FLEET_CARD`/`FLEET_BOARD`
  survive into `env_json` and come back on resume. They are minted per *delegation*, not per
  start, and `run_for_card` refuses any column `env` key starting with `FLEET_` before reading
  anything, so the stored values can only ever be this daemon's own. Without this a resumed card
  child silently lost its identity and stopped refusing a self-move — and the key cannot be
  rebuilt in `agents/manager.rs`, which has no handle on `Boards` and so cannot derive a display
  key. Covered by `store::tests::a_card_childs_caller_variables_survive_but_its_token_does_not`.
- 2026-09-21 (integration) — `card_worktree_error` names the host again
  (`automation is unavailable on a worktree owned by host {host}`, contracts §1.7 and
  `BOARD.md` §11.4). `Worktrees::path` refuses a hosted worktree without saying whose it is, so
  `Worktrees::host_of` was added beside it and is read only on that refusal path; a state store
  that cannot answer leaves the older `another host` wording rather than failing the refusal.

## Follow-ups
