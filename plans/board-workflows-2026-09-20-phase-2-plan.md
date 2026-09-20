# Board workflows, phase 2: daemon — card-called delegations — Plan
> Tracker: ./board-workflows-2026-09-20-phase-2-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

Teach the delegation machinery that a caller can be a card. `Delegation.caller` becomes an untagged `DelegationCaller`, `caller_turn` and `caller_item` become `Option` (`Some` iff `Thread`), and `DeliveryState` gains `Recorded`. The agents store performs its first table rebuild, slot 007, to make three `NOT NULL` caller columns nullable and add `caller_kind`, `caller_board` and `caller_card`. `DelegationService::run_for_card` creates a depth-1 child in the board's worktree with the card footer, and the worker delivers a terminal card run through a `RunDeliveryHook` whose `Ok` is the only thing that closes the `Deliver` row. Three worker behaviours are decided rather than inherited: the caller-repair sweep skips card callers, the drain throttle keys on the board, and `wait` never consumes a card run. A peer that has not named `board.automation` sees no card-called record in `DelegationChanged` or `DelegationList`. Nothing on any board changes yet: no production path creates a card-called delegation until phase 3, and the hook slot is empty, which the worker reports as `Undeliverable { reason: "no board service" }`.

## Sizing call

**Phased, phase 2 of 9.** See ./board-workflows-2026-09-20-roadmap.md; shapes are fixed by ./board-workflows-2026-09-20-contracts.md §3.1-3.3. It is its own pull request because slot 007 is the ladder's first rebuild and is irreversible once a daemon has run it, and because the untagged-caller goldens are the proof that every shipped delegation still encodes byte for byte.

## Repository context

- Record: `Delegation` `crates/fleet-core/src/agents/delegation.rs:15` — `caller: ThreadId` :19, `caller_turn: TurnId` :21, `caller_item: ItemId` :23, `delivery` :52; `DeliveryState` :184 (`#[serde(tag = "type", content = "data")]` :183) with `Pending` :186, `Delivered` :188-192, `Consumed` :199, `Undeliverable` :201-204, `is_pending` :210, `word` :216; `DelegationStatus::is_terminal` :126.
- Service: `DelegationService` `services/agents/delegation/mod.rs:60`, `Inner` :64-79, `new` :88-112, `publish_changed` :123; `RunRequest` :133-150; submodules :19-34. Limits `limits.rs`: `MAX_LIVE_DELEGATIONS = 8` :9, `MAX_NUDGES = 2` :11, `SETTLE_GRACE` :13, `RETRY_TICK` :15.
- `run.rs:88` validation order, nine rules: caller-exists :98-102, caller-locality :104-108, running-turn :110-119, depth :121-132, live-child-limit :134-145, daemon-live-limit :147-158, provider binary :160-177, worktree resolution :187-191, transactional re-check in `store/delegations.rs::reserve` :341-374 (refusal → Conflict `run.rs:293-295`). Token :194-199; `manager.create_with(CreateOptions { … })` :297-313; caller transcript append :331-345; first message :353-375; compensation :385, :401-440. `FLEET_OWNED_CHILD_ENV` :36, overwrite-with-warn :209-222.
- `footer.rs`: `FOOTER_TEMPLATE` :13-21, substitution order `{fleet}` → `{id}` → `{expectation}` :41-44, `footer()` :38, `first_message()` :48-55, `child_title` :87-96 (`↳ {provider} — {first 48}`), verbatim tests :159-249.
- `worker.rs`: `drain` :30, `repair_missing_callers` :57 and :126-146, per-caller throttle :25-29 enforced :82-84, `deliver` :384 with item patch :392-413, `Consumed` short-circuit :420-428, open gate :447-449, `should_send` :451-481, never marks done on send :505-514, `make_undeliverable` :635-656, `finish_row` :623. Store predicate `mark_missing_callers_undeliverable` `store/delegations.rs:660-691`.
- `transition.rs` file doc :1-5: the SQL half calls `child_transition` inside the writer's transaction, so a manager call from that file deadlocks the writer. Call site `store/delegations.rs:106-168`; the `Deliver` row is closed there at :161 for a thread caller.
- `queries.rs`: `wait` :352-397, `consume_for` :443-475 (consumes only when terminal and `caller == Some(current.caller)` :448), `list` :329, `get` :341.
- Store: slot 003 DDL `store/migrations.rs:140-162`, slot 004 :260, slot 006 :301; ladder `MIGRATIONS` :51-94 with a pinned `sha256` per slot; **rules 1-6 at :3-22** (immutable slots, sha tripwire, guarded re-run, a ceiling for tests, a why-comment, a database ahead of the build is refused :20-22); runner :324. Goldens `schema::REQUIRED_DELEGATION_COLUMNS` `schema.rs:372-402` (29 names, consumed `migrations.rs:501`, `delegation_columns_at_slot` :662-668), `REQUIRED_TABLES` :350, `REQUIRED_INDEXES` :411. Read projection golden `DELEGATION_COLUMNS` `store/delegations.rs:701-704`; `RawDelegation::read` :878, `decode` :908, `EncodedDelegation::from_delegation` :811, `insert` :248-261. **No table-rebuild precedent exists** (rule 4, :13-14).
- Filter: `event_visible` `server/connection/events.rs:18-48`, the delegation arm at :42; `event_kind` :50-79. **No capability filtering of list responses exists**; request gating today is only "is there a delegation service" (`dispatch.rs:752-758`).
- Goldens: `crates/fleet-proto/tests/agent_compatibility.rs` with `assert_frame` `tests/support/mod.rs:16-42`; the legacy fixtures in `tests/agent_compatibility/legacy.rs`.
- Skills to load first: `rust-ipc-protocol` (the untagged shape, the filter, the goldens), `rust-workspace-architecture` (the migration slot, the error policy, the ADR-free doc rule), `rust-async-background-work` (the worker and the hook), `rust-gpui-testing` (the tokio tests), `zed-quality-review` before calling the phase done.
- Docs to keep in step: `docs/NATIVE-AGENTS.md` §15 (:1677; §15.2 delivery :1816, exactly-once boundary :1847-1849, `wait` consumption :1855-1858; §15.6 ends at :2056) and `docs/research/agents-contracts.md` Delegations :43-109.

## Assumptions

- `BoardId` and `CardId` come from `fleet_core::board`; `agents::delegation` is the same crate, so no new crate edge appears (roadmap, seam 1 → 2).
- `run_for_card` writes `depth = 1` directly. There is no caller thread, so the depth-limit rule has nothing to read (contracts §3.3).
- A card caller has no caller turn and no caller item, so `run_for_card` skips the caller-transcript append (`run.rs:331-345`) entirely, and `transition.rs` needs no change beyond the caller type: it already routes ending through an outbox row.
- The display key (`FLT-7`) is not derivable from a `CardId`, so `CardRunRequest` carries `key: String` in addition to `card` (contracts §3.3). Phase 3 fills it from `Card::display_key`.
- `run_for_card` is `pub(crate)` and has no caller in this phase; `#[cfg(test)]` use plus the new tests are what keep it alive under `-D warnings`.
- The slot-007 rebuild runs inside the ladder's existing transaction and copies rows with an explicit column list, never `SELECT *`.

## Out of scope

- `Boards`, `Automation`, `start_for_card`, `on_run_delivered`'s board write, `resume_automation`, the slot-release subscriber: phase 3. This phase ships only the trait and the call site.
- `Checkpoints::changed_since` and `result_files` for card runs: phase 4.
- Advertising `board.automation` and the three run requests: phase 3. The filter added here therefore hides every card-called record from every peer, which is correct until then.

## Affected areas

- `crates/fleet-core/src/agents/delegation.rs` — `DelegationCaller`, the three caller fields, `DeliveryState::Recorded`.
- `crates/fleet-daemon/src/services/agents/store/migrations.rs` (slot 007), `store/schema.rs` (three goldens), `store/delegations.rs` (`DELEGATION_COLUMNS`, `insert`, `decode`, `EncodedDelegation`, `mark_missing_callers_undeliverable`, two new live queries).
- `services/agents/delegation/{mod.rs,run.rs,footer.rs,worker.rs,queries.rs,complete.rs,cancel.rs}` and new `delegation/tests/card.rs`.
- `crates/fleet-daemon/src/server/connection.rs` and `server/connection/events.rs`.
- `crates/fleet-proto/tests/agent_compatibility.rs`.
- `docs/NATIVE-AGENTS.md` (new §15.7), `docs/research/agents-contracts.md`.

## Tasks

### P2-T01 — `DelegationCaller`, optional caller turn and item, `DeliveryState::Recorded`
- **Intent:** One record shape that carries either caller, with a thread caller encoding byte for byte as today.
- **Touches:** `crates/fleet-core/src/agents/delegation.rs`, every match the change breaks.
- **Steps:**
  - `#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)] #[serde(untagged)] pub enum DelegationCaller { Thread(ThreadId), Card { board: BoardId, card: CardId } }` with `thread()`, `card()` and `is_card()` as contracts §3.1. `Card` carries `#[serde(rename_all = "camelCase")]` so the map keys are `board` and `card`.
  - `Delegation.caller: DelegationCaller`; `caller_turn: Option<TurnId>` and `caller_item: Option<ItemId>`, both `#[serde(default, skip_serializing_if = "Option::is_none")]`, with a doc comment "`Some` iff the caller is a thread".
  - `DeliveryState::Recorded` — a card caller's board write has committed. It serialises as `{"type":"recorded"}`, `word()` answers `"recorded"`, and `is_pending()` is false. Add a doc comment that `Recorded` is terminal for delivery and is never overwritten by the repair sweep.
  - Sweep every `delegation.caller` reader: `cargo check --workspace --all-targets`, then `grep -rn "\.caller" crates/ | grep -i delegation`. Known readers to fix: `worker.rs:82-84` (the throttle), `queries.rs:448` (`consume_for`), `store/delegations.rs` encode/decode, `fleet-cli/src/envelope.rs:107-119` (the JSON caller), `fleet-app/src/state/harness.rs:134` filled at `harness/projection.rs:510`. The CLI human line has no caller column (`fleet-cli/src/human.rs:200-218`) and needs none.
  - Tests in `delegation.rs`: a thread-called record serialises with `caller` as a bare string and both caller keys present; a card-called record serialises `caller` as `{"board":…,"card":…}` with neither caller key; both round-trip; `Recorded` round-trips.
- **Verification:** `cargo test -p fleet-core delegation`, `cargo check --workspace --all-targets`.
- **Done when:** Both caller shapes round-trip and the workspace compiles.

### P2-T02 — Slot 007: the agents store's first table rebuild
- **Intent:** Make the three caller columns nullable and add the card caller's three, without editing a shipped slot.
- **Touches:** `store/migrations.rs`, `store/schema.rs`, `store/delegations.rs`.
- **Steps:**
  - Re-read rules 1 to 6 at `migrations.rs:3-22` before writing anything. Slot 007, name `delegation_card_callers`, `source` the SQL below verbatim, `sha256` computed from that source, and a why-comment on the slot: this is the ladder's first rebuild, because SQLite cannot drop `NOT NULL` from `caller_thread`, `caller_turn` and `caller_item`.
  - The source creates `delegations_v7` with `caller_kind TEXT NOT NULL DEFAULT 'thread'`, `caller_thread TEXT`, `caller_turn TEXT`, `caller_item TEXT`, `caller_board TEXT`, `caller_card TEXT` and **every other column verbatim from slots 003, 004 and 006** including `child_thread TEXT NOT NULL UNIQUE` and `env_json`; `INSERT INTO delegations_v7 (<explicit column list>) SELECT 'thread', <same list> FROM delegations;`; `DROP TABLE delegations;`; `ALTER TABLE delegations_v7 RENAME TO delegations;`; then `CREATE INDEX idx_delegations_caller ON delegations(caller_thread, created);` and `CREATE INDEX idx_delegations_card ON delegations(caller_board, caller_card);`. `delegation_outbox` is untouched.
  - Guard (rule 4): the slot's runner checks `PRAGMA table_info(delegations)` for `caller_kind` and returns without doing anything when it is already there, so a re-run is a no-op.
  - Goldens: `REQUIRED_DELEGATION_COLUMNS` (`schema.rs:372-402`) grows by three; `REQUIRED_INDEXES` (:411) grows by `idx_delegations_card` (contracts §3.2); `DELEGATION_COLUMNS` (`delegations.rs:701-704`) grows by three.
  - `EncodedDelegation::from_delegation` (:811) writes `caller_kind` plus the matching columns; `insert` (:248-261) enforces the constraint in Rust — `'thread'` requires all three thread columns non-null, `'card'` requires board and card non-null — refusing with a `DaemonError` rather than a panic. `decode` (:908) reconstructs the enum and leaves `caller_turn`/`caller_item` `None` for a card.
  - Add `live_for_board(board)` and `live_for_card(board, card)` to `store/delegations.rs` beside `live` (:530), both seeking `idx_delegations_card`.
  - Tests in `migrations.rs`, mirroring the shipped slot tests: `slot_007_rebuilds_delegations_with_a_caller_kind_on_the_slot_006_schema`, `slot_007_preserves_every_delegation_row_it_rebuilds` (insert a slot-006 row, run 007, assert every column and `caller_kind = 'thread'`), `slot_007_is_a_no_op_when_caller_kind_already_exists`. Keep `the_full_ladder_applies_cleanly_from_empty` (:505), `the_ladder_is_idempotent` (:516), `every_index_the_read_queries_rely_on_exists` (:530), `the_read_paths_are_index_seeks_not_scans` (:543) and `every_slot_hash_matches_its_source` (:806) green, and add a store test that a card-called row round-trips through `insert`/`get`/`decode`.
- **Verification:** `cargo test -p fleet-daemon --lib services::agents::store`.
- **Done when:** A slot-006 database reaches 007 with every row intact and a second run changes nothing.

### P2-T03 — `CardRunRequest`, `run_for_card`, the card footer and the child environment
- **Intent:** Create a depth-1 child for a card, with the card's own footer and env, reusing every rule that still applies.
- **Touches:** `delegation/mod.rs`, `delegation/run.rs`, `delegation/footer.rs`.
- **Steps:**
  - `CardRunRequest` in `mod.rs` beside `RunRequest` (:133), exactly as contracts §3.3 plus `key: String` (the card's display key). `pub(crate) async fn run_for_card(&self, request: CardRunRequest) -> DaemonResult<(Delegation, Option<String>)>` in `run.rs`, returning the same pair as `run`.
  - Rules, spelled out against `run.rs:88`'s order. **Skipped:** caller-exists (:98-102), caller-locality (:104-108), running-turn (:110-119), depth (:121-132 — `depth = 1` is written directly), live-child-limit per caller (:134-145). **Kept:** daemon-live-limit (:147-158), provider binary (:160-177), worktree resolution (:187-191 — the worktree is named by the request, so there is no "defaults to the caller's" path and no `SAME_WORKTREE_WARNING`), and the transactional re-check in `reserve` (`store/delegations.rs:341-374`) whose refusal still maps to `Conflict` (`run.rs:293-295`). **Added:** a worktree-host rule — when the resolved worktree has `host.is_some()` (`Worktree.host`, `crates/fleet-core/src/model.rs:143`), refuse `Unsupported("automation is unavailable on a worktree owned by host {host}")` before minting anything (contracts §3.3).
  - Token minting (:194-199), `manager.create_with` (:297-313) with `parent: None`, `title` from the request, and the compensation paths (:385, :401-440) are unchanged. The caller-transcript append (:331-345) is **skipped**: there is no caller turn or item.
  - `footer.rs` gains `CARD_FOOTER_TEMPLATE` verbatim from contracts §3.3 — ten lines, from `--- Fleet run {id} for card {key} on board {board} ---` to `Do not move card {key}; its report moves it when you finish. You may comment on it and move other cards.` — plus `card_footer(id, key, board, expectation, fleet)` substituting `{fleet}` → `{id}` → `{key}` → `{board}` → `{expectation}` in that order, and `card_first_message(brief, …)` joining brief and footer with one blank line the way `first_message` (:48-55) does. An empty expectation prints `The card expects: (the column names no expectation)`.
  - `card_child_title(key, title) -> String` beside `child_title` (:87-96): `↳ {key} — {title cut at 48}`, the same char-boundary cut.
  - Environment: reject any `CardRunRequest.env` key starting with `FLEET_` before creating anything; the daemon adds `FLEET_CARD={key}` and `FLEET_BOARD={board}` to `extra_env` beside `FLEET_DELEGATION` and `FLEET_DELEGATION_TOKEN`, and adds both names to `FLEET_OWNED_CHILD_ENV` (`run.rs:36`) so the overwrite-with-warn path (:209-222) covers them.
  - Verbatim footer tests beside the existing ones (:159-249): the rendered card footer matches the template character for character; the thread footer is unchanged; an empty expectation renders the parenthetical.
- **Verification:** `cargo test -p fleet-daemon --lib services::agents::delegation::footer`.
- **Done when:** The card footer is byte-exact and `run_for_card` compiles against every kept rule.

### P2-T04 — `RunDeliveryHook` and the three decided worker behaviours
- **Intent:** Deliver a terminal card run through the board service, exactly once, without the transition half ever touching a board.
- **Touches:** `delegation/mod.rs`, `delegation/worker.rs`, `delegation/queries.rs`, `store/delegations.rs`.
- **Steps:**
  - `#[async_trait::async_trait] pub(crate) trait RunDeliveryHook: Send + Sync { async fn on_run_delivered(&self, board: &BoardId, card: &CardId, delegation: &Delegation) -> DaemonResult<()>; }` in `mod.rs`, with the doc comment from contracts §3.3, plus `Inner.run_hook: Mutex<Option<Weak<dyn RunDeliveryHook>>>` and `pub(crate) fn set_run_delivery_hook(&self, hook: Weak<dyn RunDeliveryHook>)`.
  - `pub(crate) async fn live_for_board(&self, board)` and `live_for_card(&self, board, card)` on the service, over the store queries from P2-T02.
  - `deliver` (`worker.rs:384`) branches on `delegation.caller.is_card()`. The card arm skips the item patch (:392-413), the `Consumed` short-circuit (:420-428), the open-gate check (:447-449), `should_send` (:451-481) and the durable-submission machinery; it upgrades the `Weak`, calls `on_run_delivered`, and on `Ok` sets `delivery = Recorded` and closes the row with `mark_done_for(.., Deliver, ..)`; on `Err` it leaves the row open and logs, so the next drain retries; when the hook is unset or the `Weak` fails to upgrade it calls `make_undeliverable` (:635-656) with the reason `no board service`.
  - The three decided behaviours: `mark_missing_callers_undeliverable` (`store/delegations.rs:660`) gains `AND caller_kind = 'thread'` in its predicate (:666-672), so the sweep never touches a card run; the drain throttle (`worker.rs:25-29`, enforced :82-84) keys on `enum ThrottleKey { Thread(ThreadId), Board(BoardId) }` derived from the caller, so two cards on one board still serialise but two boards do not; `consume_for` (`queries.rs:443-475`) consumes only when the caller is `Thread(t)` and `t == caller`, so `wait` on a card run reads the record and leaves `delivery` alone (:448).
  - `transition.rs` is not given a manager or a board call in any arm — re-read its file doc (:1-5) and leave the SQL half at `store/delegations.rs:106-168` closing `Deliver` rows for thread callers only.
- **Verification:** `cargo test -p fleet-daemon --lib services::agents::delegation`.
- **Done when:** A terminal card run's `Deliver` row closes only after the hook returns `Ok`, and no board call exists below `transition.rs`.

### P2-T05 — The per-peer event and list filter
- **Intent:** A peer that cannot name `board.automation` never learns that card-called delegations exist.
- **Touches:** `server/connection/events.rs`, `server/connection.rs`.
- **Steps:**
  - In `event_visible` (`events.rs:18-48`) add, **before** the existing arm at :42, `Event::DelegationChanged(d) if d.caller.is_card() => client.supports(BOARD_AUTOMATION_CAPABILITY)`. `event_kind` (:50-79) is unchanged; a card-called change still rides `EventKind::AgentSummary`.
  - There is no precedent for filtering a list response, so locate the single place the connection writes a response — start from `write_response` (`connection.rs:908-946`) and confirm with `grep -n "ResponseBody::Delegations" crates/fleet-daemon/src` — and filter `ResponseBody::Delegations(v)` there with the same predicate, retaining every record whose caller is a thread when the peer has not named the capability. Put the why-comment beside it: a listing is a discovery surface, an explicit id is not.
  - `ResponseBody::Delegation(_)` (from `DelegationGet`) is **not** filtered: an explicit id is answered to any peer that named `agent.delegation`.
  - Tests beside the existing filter tests (`events.rs:110-156`): a peer with `agent.delegation` and without `board.automation` sees a thread-called `DelegationChanged` and not a card-called one; a peer with both sees both; a `Delegations` response to the first peer contains only thread callers.
- **Verification:** `cargo test -p fleet-daemon --lib server::connection`.
- **Done when:** No peer in this phase can observe a card-called record through a list or an event.

### P2-T06 — Goldens, `delegation/tests/card.rs` and the documents
- **Intent:** Prove the wire did not move, prove the lifecycle end to end, and write both facts down.
- **Touches:** `crates/fleet-proto/tests/agent_compatibility.rs`, new `services/agents/delegation/tests/card.rs`, `delegation/tests/mod.rs`, `docs/NATIVE-AGENTS.md`, `docs/research/agents-contracts.md`.
- **Steps:**
  - Goldens: add exactly one card-caller fixture through `assert_frame` — a `Delegation` with `caller: { board, card }`, no `callerTurn`, no `callerItem`, and `delivery: {"type":"recorded"}` — and **do not edit** the shipped thread-caller fixture or anything in `tests/agent_compatibility/legacy.rs`. A byte diff in either is a missing `skip_serializing_if`, never a fixture to refresh.
  - New `delegation/tests/card.rs`, `#[tokio::test]`s against the scripted fake harness the sibling tests already use: `a_card_called_delegation_completes_and_records_through_the_hook` (a child created by `run_for_card` reports with `complete`, the record goes `Succeeded`, the test hook records the board and card once, and only then is the `Deliver` row done and the delivery `Recorded`); `an_unset_hook_makes_a_card_delivery_undeliverable` (no hook installed ⇒ `Undeliverable { reason: "no board service" }`); `a_failing_hook_leaves_the_deliver_row_open_for_the_next_drain` (the hook returns `Err` once, then `Ok`, and the hook is called twice while the board write is recorded once); `the_repair_sweep_ignores_card_callers` (a terminal card run with `delivery = pending` survives a drain that would have marked a thread run `caller deleted`); `wait_never_consumes_a_card_run` (`wait` on a terminal card run returns it and leaves `delivery` unchanged); `the_drain_throttle_keys_card_runs_on_their_board` (two open rows for one board drain one per pass, two boards drain together); `a_card_run_carries_no_caller_turn_or_item` (the stored record and the caller's transcript both show it).
  - `a_peer_without_the_capability_never_sees_a_card_caller` lives with the P2-T05 connection tests, not here; name it in the tracker under P2-T05.
  - `docs/NATIVE-AGENTS.md` §15 gains **§15.7 "Card callers"** after §15.6 (:2056): the untagged caller, the optional caller turn and item, `Recorded` and where it sits in the delivery table of §15.2 (:1841), slot 007 as the first rebuild and why, the hook and its exactly-once boundary stated in the same words as :1847-1849, the three decided behaviours, the capability filter, and the sentence that no production path creates one until phase 3.
  - `docs/research/agents-contracts.md` Delegations (:43-109): update the struct, `DeliveryState` (:98-102) and the store column list (:504-569) to the shipped shapes.
- **Verification:** `cargo test -p fleet-proto --test agent_compatibility`, `cargo test -p fleet-daemon --lib services::agents::delegation::tests::card`.
- **Done when:** The legacy fixture parses untouched and every test above is green.

## Verification

```sh
make lint
cargo test -p fleet-core delegation
cargo test -p fleet-proto --test agent_compatibility
cargo test -p fleet-daemon
make test
```

No screen changes, so `make harness` is **not** required for this phase. Run `make restart` at the end so the running `fleetd` opens the agents database at slot 007 before phase 3 starts.

## Definition of done

- [ ] Every task above is `[x]` in the tracker.
- [ ] `make lint` and `cargo check --workspace --all-targets` are clean.
- [ ] `make test` passes; the shipped thread-caller golden and every legacy fixture are byte-identical.
- [ ] A slot-006 database reaches slot 007 with every row preserved, and re-running the ladder changes nothing.
- [ ] A terminal card run's `Deliver` row closes only after the hook returns `Ok`; an unset hook yields `Undeliverable { reason: "no board service" }`.
- [ ] No code below `transition.rs` calls the manager or a board.
- [ ] `docs/NATIVE-AGENTS.md` §15.7 and `docs/research/agents-contracts.md` ride with the code.

## Risks and rollback

- **Slot 007 is irreversible.** A database at 007 is refused by an earlier daemon (`migrations.rs:20-22`). Rolling this phase back after a daemon has run it means deleting the agents database or restoring a backup. Say so in the PR.
- **A rebuild can silently drop a column.** Write the `INSERT … SELECT` with an explicit column list in both positions and let `slot_007_preserves_every_delegation_row_it_rebuilds` assert every one of them; never `SELECT *`.
- **Untagged enums are decided by shape.** If `ThreadId` ever stops serialising as a bare string, `DelegationCaller` becomes ambiguous. The two goldens are the tripwire.
- **The gate between the worker and the board is a `Weak`.** A hook that outlives its `Arc` degrades to `Undeliverable`, not to a panic, and a phase-2 test asserts exactly that.
- **`wait` and the repair sweep are shared with hand-started subagents.** Both changes are predicated on `caller_kind`, so a regression shows up as a thread-caller test failing, not as a silent behaviour change.

## Contract resolutions

The gaps this plan's first draft found were resolved in the contracts file on 2026-09-20; the
contracts wording wins over any earlier phrasing above.

1. `CardRunRequest` carries `key: String` (the display key); phase 3 fills it from `Card::display_key` (contracts §3.3).
2. `run_for_card` writes `depth = 1` directly and skips the depth-limit rule (contracts §3.3).
3. The caller-transcript append (run.rs:331-345) is skipped for a card caller (contracts §3.3).
4. `DeliveryState::Recorded` has `is_pending() == false`; the repair sweep's predicate (`delivery = 'pending' AND caller_kind = 'thread'`) never reaches it (contracts §3.1).
5. `FLEET_CARD` and `FLEET_BOARD` join `FLEET_OWNED_CHILD_ENV` (contracts §3.3).
6. `REQUIRED_INDEXES` grows by `idx_delegations_card` (contracts §3.2).
7. `live_for_board`/`live_for_card` have store verbs in `store/delegations.rs` beside `live` (contracts §3.2).
8. `run_for_card` adds a worktree-host rule refusing `automation is unavailable on a worktree owned by host {host}` as `Unsupported`; the board-side refusal in phase 3 is the second line of defence (contracts §3.3).
