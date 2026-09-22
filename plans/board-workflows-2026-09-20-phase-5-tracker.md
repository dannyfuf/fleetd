# Board workflows, phase 5: columns, card fields and run verbs — Tracker
> Plan: ./board-workflows-2026-09-20-phase-5-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.
- Definition of done is not met until every box is ticked and this tracker matches reality.

## Kickoff
- [x] I have read the plan end to end.
- [ ] I have run the project-wide verification commands once on a clean tree to confirm a green baseline.
  (Not tickable by any phase-5 owner: the tree was never clean — five agents built this phase in
  one worktree — and `make lint`/`make test` are the integration stage's, not a task's.)
- [x] I am ready to start.

## Tasks
- [x] P5-T01 — The argument surface
  - verified: every flag, verb, default and conflict of the plan and contracts §4 parses —
    checked with a throwaway `#[test]` (`columns preset workflow`, `columns move` with neither
    and with both of `--after`/`--before`, `--instructions`/`--instructions-file`,
    `--desc`/`--desc-file`, `--provider`/`--clear-agent`, an unknown `--provider` value,
    `card new --blocked-by/--blocks`, `card wait` default 540, `card move --cancel-run`,
    `board set --max-live-runs`), then deleted so the tests file stays the tests owner's.
    `cargo test -p fleet-cli` 137+9 passed; `cargo clippy -p fleet-cli --all-targets
    --all-features -- -D warnings` clean.
  - verified (P5-T06's owner, the tests the surface was missing): `mod parsing` now keeps them —
    `parses_the_columns_family_and_every_flag_its_verbs_take` (the verb-less listing, every
    category word, both `--after`/`--before` forms, the whole `edit` flag set and its clearing
    half, `move`, `remove --move-cards-to`, `preset workflow`),
    `parses_the_five_run_verbs_and_the_waits_default_timeout` (540 and an explicit one),
    `parses_the_card_agent_link_and_run_flags`, `parses_board_set_max_live_runs`, and
    `refuses_the_workflow_flag_pairs_that_contradict_each_other` (31 argument vectors clap must
    reject). `cargo test -p fleet-cli` 187 + 9 + 0 passed.
- [x] P5-T02 — The `columns` family
  - verified: `cargo test -p fleet-cli board::columns` — 23 unit tests over the local vector
    edits (add's slug/position/refusals, edit's action, routes, env and `--instructions-file`,
    move in both directions, remove's card moves including archived ones, the preset's two
    notes, the table's cells) all pass; `cargo check -p fleet-cli --all-targets` and
    `cargo clippy -p fleet-cli --all-targets --all-features -- -D warnings` are clean for the
    owned files.
- [x] P5-T03 — Card fields, links and the self-move refusal
  - verified: `cargo test -p fleet-cli` 171 + 9 pass and `cargo clippy -p fleet-cli
    --all-targets --all-features -- -D warnings` is clean; a throwaway `mod scratch` in
    `commands/board/` proved the refusal (fires only with `FLEET_DELEGATION` set and a key
    equal to `FLEET_CARD` case-insensitively, never for another verb or another card, and
    before any request), `--desc-file` reaching `CardPatch.description`, `board set
    --max-live-runs 3` reaching `BoardPatch.settings`, the agent flags building
    `Some(Some(..))` / `Some(None)` and merging onto the card's own block, and the link flags
    computing the whole `blocked_by` set — then deleted, the tests file staying P5-T06's.
- [x] P5-T04 — The five run verbs
  - verified: same run; the scratch module proved the runs line while it was rendered here (ten
    tab-separated fields, the outcome word for a terminal run and the live delegation's word for
    a live one, `—` for what is unknown, `14m 02s` between two stamps, and an empty string — no
    panic — for a card with no runs), and the verb now calls `human::card_runs` instead, whose
    own tests (`human::tests`) cover the line; `card runs` and `card attach` resolve from the
    view the dispatcher already read, so neither sends a second request.
- [x] P5-T05 — Rendering and envelopes
  - verified: `cargo test -p fleet-cli` 171 + 9 + 0 pass, `cargo check -p fleet-cli
    --all-targets` and `cargo clippy -p fleet-cli --all-targets --all-features -- -D warnings`
    are clean. Ten new tests in `human.rs`'s own `mod tests` cover every mark (`● working`,
    `! needs you`, `… pending` before the priority word, `⊘ n` after the title, `Ready (2) ⚡`),
    the header (`backend: local · 2/2 working · 1 needs you`, and `backend: local` alone on a
    board running nothing), `board list`'s `WORKING`/`NEEDS YOU` cells, the ten-field run line
    on both clocks, `card show`'s Agent/Blocked by/Blocks/Runs sections and their absence on a
    card with no workflow, the ninth `caller` field in both shapes, and the epoch → RFC 3339
    round trip; `envelope.rs` asserts `liveRuns` is carried and omitted when empty.
- [x] P5-T06 — Socket tests and the help test
  - verified: `cargo test -p fleet-cli board::tests` 89 passed, `cargo test -p fleet-cli`
    187 + 9 + 0 passed, `cargo check -p fleet-cli --all-targets` and `cargo clippy -p fleet-cli
    --all-targets --all-features -- -D warnings` clean. Nine new tests in
    `commands/board/tests.rs`: five parsing tests (P5-T01's, see above) and, in
    `mod orchestration`, one scripted-socket test per new request shape — every `columns` verb's
    single `UpdateBoard { patch.statuses }`, `remove --move-cards-to`'s two `MoveCard`s *before*
    that `UpdateBoard`, a `preset workflow` with nothing to add sending no request at all,
    `MoveCard { cancel_run: true }`, `CardRunStart`, `CardRunCancel`,
    `CardRunWait { timeout_ms: 30_000 }` with exit 2 / 0 / 2 for a live, an ended and a
    never-started run, `card runs` and `card attach` answering from the view with no second
    request, `CreateCard` carrying `agent` and `blocked_by` then the `--blocks` follow-up
    `UpdateCard`, `card edit --clear-agent` as `agent: Some(None)`, `--add-blocks` patching only
    the card it names, and `UpdateBoard` with `settings.max_live_runs`. The self-move refusal is
    asserted through `refuse_self_move` itself (the sentence, `print_error` + `error_result` = 1,
    and the four cases it must let through), plus a socket test that the same move of *another*
    card still reaches the daemon. `board_help_lists_the_new_commands_and_their_flags` now names
    `columns`, its five verbs, the run verbs and every new card, column and board flag.
- [x] P5-T07 — The smoke script, the Make target, the skills and the doc
  - verified: `make smoke-workflow` twice in a row from this tree, green both times — the
    preset applies, four linked cards run eight scripted agents and every card reaches Done,
    `card wait` on the last exits 0 and `board show --json` shows four cards in `done`.
    `pgrep -x fleetd` counted 129 before and 129 after each run, and no
    `$TMPDIR/fleet-smoke-workflow.*` directory survived either: the private home, the daemon
    and the scripted agents all go with the trap. `cargo test -p fleet-cli` 187 + 9 + 0 passed,
    `cargo clippy -p fleet-cli --all-targets --all-features -- -D warnings` clean, `bash -n`
    clean on the script.

## Notes / decisions log
- 2026-09-21 (review fixes) — five CLI fixes from the three-reviewer pass:
  1. `card new --blocks KEY` rendered the dependant against the board view read *before* the
     card existed, so the new blocker printed as a raw id and `blocked()` — which calls a
     blocker it cannot find canceled — marked the dependant amber. The created card now joins
     the list it is rendered against.
  2. `columns edit` could not change one environment variable in one command: `--env` appended,
     so a repeated key hit `validate_env`'s "given twice", and `--clear-env` was
     `conflicts_with = "env"`. A repeated key now replaces, and the two flags compose
     (clear, then apply).
  3. `card attach` had a third spelling of the failed-start refusal. The CLI and the app now
     both answer `{KEY} has no run` on a card that never ran and `{KEY}'s runs never reached a
     thread` when it ran but no run of it did; contracts §3.5 is amended to those two.
  4. `card wait` answered exit 0 for a card the board still *owes* a run, reading the exit code
     off an older terminal run. An owed run is not a finished one: it exits 2.
  5. The self-move refusal ran inside `board::run`, after `run_command` had already autostarted
     a daemon — its own doc says it costs no round trip. It now sits beside
     `subagents::validate_context`, before `fleet_home()` and `ensure_daemon`.

- 2026-09-21 (contracts:cli) — the CLI skeleton landed so P5-T02 to P5-T05 can start at once.
  `commands/board/columns.rs` holds `run(client, view, arguments, json)` as a stub (owner
  `l:columns`); the board is already resolved by `board.rs`'s dispatcher, so the columns owner
  does not call `resolve_board` again. Five stub fns in `board.rs` — `card_run`,
  `card_run_cancel`, `card_runs`, `card_attach`, `card_wait` — and three ignored flag groups
  (`--blocked-by`/`--blocks`, the five link flags, `--cancel-run`) carry
  `// CONTRACT STUB (l:card-verbs)`. `board set --max-live-runs` parses and `board_patch`
  ignores it; the same owner wires it into `BoardPatch.settings`.
- 2026-09-21 (contracts:cli) — `command_requests_json` needed no new arm: `--json` is a global
  flag on `BoardArgs`, so `Command::Board(arguments) => arguments.json` already honours
  `columns` and the five run verbs. The plan's `commands.rs:263-300` line predates that.
- 2026-09-21 (contracts:cli) — `Command` took `#[allow(clippy::large_enum_variant)]` with a
  justification, following `fleet-proto/src/response.rs:262`: `card edit` is now the widest flag
  set in the CLI, and one `Command` is parsed per process and consumed immediately.

- 2026-09-21 (l:columns) — the `columns` family plans the whole vector in one pure `plan(view,
  command) -> Planned` and sends it in one `UpdateBoard`, so the arithmetic is unit-tested
  without a daemon and the socket tests (P5-T06) assert only the request. `Planned.moves` carries
  the `MoveCard`s `remove --move-cards-to` needs, archived cards included: the daemon's
  `status {id} is used by card {KEY}` refusal reads every card on the board, not just the live
  ones. Human output is the columns table; `--json` re-uses `board_show_output`, so the envelope
  stays P5-T05's to grow.
- 2026-09-21 (l:columns) — `preset workflow` prints two facts the vector alone cannot say: that
  nothing was missing, and that a preset column the board already had (the shipped `In Progress`)
  was left alone and still runs nothing, with the `columns edit` line that would fix it. Only the
  human branch prints them; a `--json` reader has the whole board.
- 2026-09-21 (l:columns) — refusals that stayed with the daemon on purpose: a bad `--env` entry, a
  route pointing backwards, automation on a context or Jira board, a column with live runs, and
  `--on-enter skill:` with no name (`a skill action needs a name`). The CLI refuses only what
  never reaches the wire.
- 2026-09-21 (l:card-verbs) — most of P5-T03's plumbing was already in the tree and was not
  rewritten: `Client::move_card` already took `cancel_run`, the three run methods and their
  capability refusal already existed (phase 3), `CardDraft.agent`/`blocked_by` and
  `CardPatch.agent`/`blocked_by` already existed (phase 1), and `--json` is global on
  `BoardArgs`, so `commands.rs` needed nothing. The task was the wiring only.
- 2026-09-21 (l:card-verbs) — `FLEET_CARD` is read through `Environment::card_from_process()`
  rather than a fourth `Environment` field: the field would have broken four exhaustive
  `Environment { .. }` literals in `commands/tests.rs`, a file this owner may not edit, and
  nothing validates the variable — one verb compares it to a key and the rest ignore it.
  `FLEET_BOARD` is not read at all: the CLI resolves a board from `--board`/`--worktree`/
  `--context`, so an unread field would be dead code under `-D warnings`. Both variables are
  still the daemon's to set and P5-T07's to document in `fleet-subagent-cli`.
- 2026-09-21 (l:card-verbs) — `card edit --model M` merges onto the card's existing agent block
  instead of replacing it: `apply_card_patch` sets `agent` wholesale, so a replacement would
  silently drop the provider the card already asks for. `--clear-agent` still sends
  `Some(None)`, and the whole `blocked_by` vector is still computed CLI-side.
- 2026-09-21 (l:card-verbs) — `card edit --add-blocks KEY` alone is not an empty patch: it
  changes the card it names, not this one, so the `card edit requires at least one field or
  --archive` refusal fires only when neither this card's patch nor the blocks flags would send
  anything. Both verbs print every card `--blocks` changed after the card the verb was about;
  `--json` stays one `BoardCardEnvelope`, since the linked cards are an effect, not the subject.
- 2026-09-21 (l:card-verbs) — `card run` prints the pending sentence rather than a run line when
  the board is at its `max_live_runs` ceiling and the daemon owed the card a run instead of
  starting one; `card wait` exits 2 for a live newest run and for a card with no run at all.
- 2026-09-21 (l:render) — `human::board` and `human::board_card` kept their signatures, so the
  clock the counts need is derived inside the renderer: `human::rfc3339_utc(epoch)` (public, the
  inverse of the file's `epoch_secs`) turns the epoch seconds `board show` already passes into
  the RFC 3339 stamp `summarize`/`attention` compare, and the header's `summarize(&[], "")` —
  which counted no run and parsed no stamp — is gone. `card show` has no clock and no run join,
  so its Runs section prints the state and an em-dash duration for a run still going; `card
  runs` passes `Some(now_epoch())` and the view's join, and both render through the one
  `human::card_runs`.
- 2026-09-21 (l:render) — `BoardEnvelope` grew `live_runs` (`liveRuns`, omitted when empty) plus
  `BoardEnvelope::from_view(view)`, which is now the only way the CLI builds one: a literal that
  forgets the join ships a board whose runs disappeared. The two existing literals were changed
  to match (see DEVIATIONS).
- 2026-09-21 (l:render) — the ninth `caller` field needs a display key a renderer cannot look up,
  so `human::subagents`/`subagent_status` kept their signatures and gained
  `subagents_with_keys`/`subagent_status_with_keys` taking `human::CallerKeys` (card id → key).
  Until `commands/subagents.rs` resolves them through one `GetBoard`, a card caller prints
  `card {card id}`, which every `fleet board card` verb still accepts as a selector.
- 2026-09-21 (l:render) — workflow rows appear only when the card has something to say through
  them (agent prefs or an automated column, a link in either direction, a run): a board nobody
  has automated prints exactly the `card show` it printed before this phase, rather than three
  new em dashes on every card.
- 2026-09-21 (l:tests) — the self-move refusal is asserted as the pure function it is
  (`refuse_self_move(&command, delegation, card)`), not by exporting `FLEET_DELEGATION` and
  `FLEET_CARD` around a socket run: no test in this workspace sets an environment variable, and
  `std::env::set_var` races every other `Environment::from_process()` in a test binary whose
  cases run in parallel. The test still pins what the contract asks for — the sentence, the
  exit code (through `commands::print_error` + `commands::error_result`, which are visible to
  this descendant module) and the four cases that must pass — and `board()` calls the refusal
  above `resolve_board`, so the refused path builds no request. A companion socket test proves
  the complement: the same `card move` of another card still reaches the daemon.
- 2026-09-21 (l:tests) — the three writing run verbs need the handshake to carry
  `board.automation` (`fleet-client` refuses without it), so their socket test authenticates
  through `run_with_capabilities`; the last case there keeps the capability away and asserts the
  client's own sentence with only the dispatcher's `GetBoard` on the wire. `card runs` and
  `card attach` need no capability: they never leave the view.
- 2026-09-21 (l:tests) — the columns socket tests assert the request only. The vector arithmetic
  is `columns/tests.rs`'s and the preset's is `fleet-core`'s, so `preset workflow`'s expectation
  is built with `apply_workflow_preset` and then pinned by naming the seven column ids it must
  produce, rather than by a second hand-written copy of the preset.
- 2026-09-21 (i:smoke) — **the smoke script has to give `in-progress` its action itself.** The
  plan's "preset → four cards → moves" would have proved nothing: `EnsureWorktreeBoard` builds
  the board from `default_statuses()`, which already owns `in-progress`, and the preset never
  rewrites a column that exists (docs/BOARD.md §11.6). The first run of the script left AGE-1
  standing in a bare In Progress with no run at all. So it runs one `columns edit in-progress
  --on-enter prompt --on-success in-review` after the preset — which is the edit every such
  board's owner has to make, and now the edit the skills tell them to make.
- 2026-09-21 (i:smoke) — **both vendors are shimmed, not just Codex.** The preset's `in-review`
  is a skill action, and `resolve_prefs` sends a skill to Claude whatever the card asks for, so
  a Codex-only shim made every second run fail. One transcript serves both: the player speaks
  whichever protocol its `--provider` names.
- 2026-09-21 (i:smoke) — **`card wait` is not a barrier for a chain, and the script does not
  pretend otherwise.** `wait_run` waits for the card's *live* run and returns at once when none
  is live, so the plan's "`card wait <last> --timeout 300`, assert 0" would have exited 2 the
  moment it was called: the last card has no run until its blockers are done. The script polls
  the board until every card is in Done and *then* asserts `card wait` exits 0 — the assertion
  the plan wanted, made at the moment it means something. The skill says the same in so many
  words, because an orchestrator will otherwise write the loop the plan described.
- 2026-09-21 (i:smoke) — the head of a chain is moved into the action column by hand. Rule 2 of
  the engine advances the *dependants* of a visited card, so a card nothing blocks is released
  by nothing; "one card move per card" is still true, but the last of the four goes to
  `in-progress` rather than `ready`. `crates/fleet-daemon/tests/boards_automation.rs` found the
  same thing in phase 3.
- 2026-09-21 (i:smoke) — the JSON assertions anchor on `"description":"","statusId":"`, not on
  `"statusId":"` alone: every `CardRun` carries a `statusId` of its own, so the bare pattern
  reads the last run's column instead of the card's. That cost one 300-second timeout on a
  chain that had in fact finished.

## Follow-ups

- ~~(l:render, for `commands/subagents.rs`'s owner) `list` and `status` should resolve a card
  caller's display key through one `GetBoard`.~~ **Closed 2026-09-21 (integration):** both verbs
  now build a `human::CallerKeys` through a private `caller_keys(client, delegations)` — one
  `GetBoard` per *distinct* board named by a card caller, and a board that cannot be read leaves
  its cards printing the id they already would have, because a listing must not fail on its own
  chrome. The key-less `human::subagents`/`subagent_status` wrappers were deleted with their last
  callers; `subagents_with_keys`/`subagent_status_with_keys` are the only renderers.
- ~~(l:render, for `commands/board.rs`'s owner) `sync` still calls `summarize(&view.board,
  &view.cards, &[], "")`, so the `BoardSyncEnvelope`'s counts are always zero.~~ **Closed
  2026-09-21 (i:smoke)**, which owns the whole crate at that point: it now passes
  `&view.live_runs` and `human::rfc3339_utc(now_epoch())`.
- ~~(l:render, for P5-T07) `docs/BOARD.md` §6 documents no envelope today.~~ **Closed 2026-09-21
  (i:smoke)**: §6's notes now name `liveRuns` and say it is omitted when nothing is live, and
  `fleet-board-planning/references/commands.md` carries the same in its envelope table.
- ~~(i:smoke, for whoever closes phase 5) `scripts/board-workflow-smoke.sh` is not in `make ci`.~~
  **Decided 2026-09-21 (integration): it is now.** `ci: lint test test-scripts smoke-workflow`.
  It costs about three seconds after a build and it is the only end-to-end proof that a column's
  action reaches a provider and moves a card, which is the whole feature; a chain that breaks
  silently in CI is the failure this target exists to catch.
