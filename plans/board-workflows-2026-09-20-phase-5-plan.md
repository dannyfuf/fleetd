# Board workflows, phase 5: columns, card fields and run verbs — Plan
> Tracker: ./board-workflows-2026-09-20-phase-5-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you
  commit.

## Summary

The CLI becomes the feature's first real user. `fleet board columns` gains list, add, edit, move,
remove and `preset workflow`, each sending one `UpdateBoard` with the full `statuses` vector. `card
new` and `card edit` learn `--provider/--model/--effort/--clear-agent`, the `blocked-by`/`blocks`
flags and `--desc-file`; `board set` learns `--max-live-runs`; `card move` learns `--cancel-run` and
refuses to move the run's own card. Five run verbs — `run`, `cancel`, `runs`, `attach`, `wait` — sit
on phase 3's three requests, `wait` exiting 0 when the newest run is terminal and 2 when it is not.
`board show` grows run marks, `⊘ n` and a `⚡` column header, `board list` grows `WORKING` and `NEEDS
YOU`, `card show` grows Agent, Blocked by, Blocks and Runs, and the envelopes grow `liveRuns` and
the `BoardEnvelope` for the `columns` family. A smoke script drives a four-card chain to Done against a private `fleetd`
with the scripted provider, and the two agent skills document it.

## Sizing call

**Phased, phase 5 of 9.** See ./board-workflows-2026-09-20-roadmap.md. Wide but shallow: one new
command module, one new args family, a dozen flags, five verbs, four renderers, two envelopes, a
shell script and two skills. No new request shape — phase 3 already serves every one of them, so a
phase-5 CLI against a phase-3 daemon works end to end. One pull request, because the smoke script is
the acceptance test for all of it.

## Repository context

- `args.rs`: `BoardCommand` `:1076-1101`, `BoardSetArgs` `:1135-1177` (no `--max-live-runs`),
  `BoardCardCommand` `:1225-1297` (no run verbs), `BoardCardFields` `:1301-1341`, `clear_flag()`
  `:1346-1354`; wait precedent `SubagentWaitArgs` `:496-519` (`--timeout` default 540 `:506`).
- `commands/board.rs` (921 lines, only `mod tests` `:921`): dispatch `:67-83`, `resolve_board`
  `:255-272`, `resolve_card` `:499-515`, `resolve_status` `:535-552`, `unique_match` `:517-533`,
  `board_patch` `:358-403`, `card_patch` `:588-636`, `card_new` `:677-710`, `card_edit` `:713-735`,
  `card_move` `:738-751`, `board_show_output` `:455-470`.
- `human.rs`: `human::board` `:441-514` (column header `:460-464`, card row `:466-471`, labels
  `:476`, assignee `:479`); `human::boards` `:356-394` (header `:357-367`, `columns()` `:405`);
  `human::board_card` `:819`; `human::subagents` `:200-218` is the tab-separated line to copy.
- `envelope.rs`: `PROTOCOL` `:17`, `BoardEnvelope` `:144-148`, `BoardCardEnvelope` `:159-162`.
  `commands.rs`: `command_requests_json` `:263-300` (board arm `:265`), `with_exit_code` `:57`,
  `FAILURE` `:36`.
- `commands/subagents.rs`: `read_text` `:490-504`, `read_error` `:506-511`, `wait` `:229-260` (exit
  0 / 2), `Environment::from_process` `:44-50`. Client `api/boards.rs`: `update_board` `:110`,
  `move_card` `:151`, plus phase 3's three run methods.
- Tests `commands/board/tests.rs`: `mod parsing` `:3`, `mod orchestration` `:1232`, driver
  `run(arguments, steps)` `:1347-1352` → `run_with_capabilities` `:1354-1384` (per-step body
  assertion `:1366-1370`), fixtures `:1257-1296`, help test `:1203`.
- Makefile `.PHONY` `:36`, binaries `:18-25`, `harness-one` `:89-92` is the target shape. Scripted
  provider: `docs/TESTING-HARNESS.md` §5, `fleet-harness agent --provider <p> --transcript <f>`
  (`fleet-harness/src/main.rs:44-61`), launcher shape `agent/launcher.rs:57-87`.
- Skills to edit: `fleet-board-planning/{SKILL.md, references/commands.md}` (card verbs `:46-61`,
  field flags `:63-75`, envelopes `:83-93`, exit codes `:105-108`);
  `fleet-subagent-cli/references/commands.md` (env table `:8-14`). Skills to load:
  `rust-workspace-architecture`, `rust-ipc-protocol`, `rust-gpui-testing`, then
  `zed-quality-review`. Doc: `docs/BOARD.md` §6 (`:647-702`, fence `:679-695`).

## Assumptions

- Phase 3 has landed and advertises `board.automation`; the client refuses the three run requests
  against an older daemon with its own sentence, which the CLI prints verbatim.
- `--effort` is free text, `--provider` is `claude|codex`, and `--clear-agent` conflicts with all
  three.
- Every columns verb is a read-modify-write of the whole `statuses` vector; a concurrent editor
  loses, exactly as `board set` already behaves.
- The self-move refusal compares the raw `<key>` argument to `FLEET_CARD`, case-insensitively,
  before connecting — so it costs no request, as §4 asks.
- `card wait` passes `--timeout` seconds × 1000 to `card_run_wait`; the client adds the 15 s grace.

## Out of scope

- Any new wire request: the columns family reuses `UpdateBoard` and `MoveCard`.
- Automation on context or Jira boards: `columns` edits order and names there, and the daemon
  refuses the rest.
- App, kit, keymap and harness work (phases 6 to 9), and `fleet subagent list`'s caller column for
  card runs (phase 2 shipped the record; the column is a phase-9 follow-up if it is missing).

## Affected areas

- `crates/fleet-cli/src/args.rs`, `commands.rs`, `commands/board.rs`, new
  `commands/board/columns.rs` and `commands/board/columns/tests.rs`, `commands/board/tests.rs`,
  `human.rs`, `envelope.rs`.
- New `scripts/board-workflow-smoke.sh`; `Makefile`.
- `.claude/skills/fleet-board-planning/`, `.claude/skills/fleet-subagent-cli/`, `docs/BOARD.md`.

## Tasks

### P5-T01 — The argument surface
- **Intent:** Every flag and verb the design's CLI blocks name, parsed and refused before any socket
  work.
- **Touches:** `crates/fleet-cli/src/args.rs`.
- **Steps:**
  - `BoardCardFields` gains `--provider <claude|codex>` (value_enum), `--model M`, `--effort E`
    (free text), `--clear-agent` (`conflicts_with_all` the three) and `--desc-file PATH`
    (`conflicts_with = "desc"`), following `clear_flag()`'s shape for the clear.
  - `BoardCardCommand::New` gains `--blocked-by KEY` and `--blocks KEY`, both `Vec<String>`; `Edit`
    gains `--add-blocked-by`, `--remove-blocked-by`, `--clear-blocked-by`, `--add-blocks`,
    `--remove-blocks`; `Move` gains `--cancel-run`.
  - Add `Run { key }`, `Cancel { key }`, `Runs { key }`, `Attach { key }`, `Wait { key, --timeout
    <u64> default_value_t = 540 }` to `BoardCardCommand`.
  - `BoardSetArgs` gains `--max-live-runs <u32>`. `BoardCommand` gains `Columns(BoardColumnsArgs)`
    holding `Option<BoardColumnsCommand>` so a bare `fleet board columns` lists.
  - `BoardColumnsCommand`: `Add { name, --id, --category, --after, --before }`, `Edit { id, --name,
    --category, --color, --on-enter <none|prompt|skill:<name>[:<args>]>, --provider, --model,
    --effort, --mode, --instructions, --instructions-file, --expect, --on-success, --no-on-success,
    --when-unblocked, --no-when-unblocked, --env K=V, --clear-env }`, `Move { id, --after, --before
    }` (one required), `Remove { id, --move-cards-to }`, `Preset { which }`.
  - Parsing tests in `board/tests.rs` `mod parsing` for each new verb and for the conflicting pairs.
- **Verification:** `cargo test -p fleet-cli board::tests::parsing`, `make lint`.
- **Done when:** Every block in the design's CLI fence parses and each conflict is a clap error, not
  a runtime one.

### P5-T02 — The `columns` family
- **Intent:** One new module that edits the column vector and sends one request per verb.
- **Touches:** new `commands/board/columns.rs`, `commands/board.rs` (dispatch and `mod columns;`).
- **Steps:**
  - `columns(args, client)`: `resolve_board` (`board.rs:255-272`), then per verb mutate a local
    `Vec<Status>` and send one `update_board(board_id, BoardPatch { statuses: Some(all), ..default
    })`.
  - `add`: id from `--id` else a slug of the name; insert at `--after`/`--before`, else last.
    `edit`: match the id exactly then case-insensitively by id or name through `unique_match`
    (`board.rs:517-533`); `--on-enter none` clears the action, `prompt` sets `ActionKind::Prompt`,
    `skill:<name>[:<args>]` sets `ActionKind::Skill`; `--instructions-file` reads through
    `read_text` (`subagents.rs:490-504`); `--env` accumulates and `--clear-env` empties;
    `--no-on-success` and `--no-when-unblocked` clear their routes. `move`: reposition by id.
  - `remove --move-cards-to ID`: one `move_card` per card in that column first, then the patch — the
    daemon refuses removing a status a card still uses and has no move-cards path of its own.
  - `preset workflow`: `fleet_core::board::defaults::apply_workflow_preset(&mut board)`; send the
    patch only when it returned `true`, else print that nothing was missing.
  - No verb prints the table: `id  name  category  on enter  on success  when unblocked`, or
    the `BoardEnvelope` under `--json` (the columns are `board.statuses`).
  - Daemon refusals (context, Jira, hosted, live runs, `max_live_runs`) are printed verbatim; the
    CLI adds no second copy of those sentences.
- **Verification:** `cargo test -p fleet-cli board::columns`, `make lint`.
- **Done when:** Each verb sends exactly one `UpdateBoard` (plus the `MoveCard`s `remove` needs) and
  the table renders.

### P5-T03 — Card fields, links and the self-move refusal
- **Intent:** The card side of the design's block, including the one refusal a run must hit.
- **Touches:** `commands/board.rs`, `commands/subagents.rs` (environment only).
- **Steps:**
  - `card_new`: `--desc-file` through `read_text`; `--provider/--model/--effort` into
    `CardDraft.agent`; `--blocked-by` resolved through `resolve_card` into `CardDraft.blocked_by`;
    `--blocks KEY` is sugar — after the card is created, send one `UpdateCard` per named card with
    `blocked_by += this card`, and print both cards.
  - `card_edit`: the agent flags patch `CardPatch.agent` (`Some(Some(..))`), `--clear-agent` sends
    `Some(None)`; the five link flags compute the whole `blocked_by` set from the current card and
    send `Some(vec)`; keep the existing empty-patch refusal `card edit requires at least one field
    or --archive` (`board.rs:728-732`).
  - `board set --max-live-runs N` rides on `BoardPatch.settings`; out-of-range values are the
    daemon's refusal, printed verbatim.
  - `card_move`: pass `--cancel-run` through to `move_card`; without it the daemon's `Conflict`
    sentence is printed verbatim.
  - Self-move refusal, before connecting: when `FLEET_DELEGATION` is set and `FLEET_CARD` equals the
    `<key>` argument case-insensitively, print `a run cannot move its own card; its report moves the
    card when it finishes` and exit 1. Read both through the `Environment::from_process` pattern
    (`subagents.rs:44-50`), extended with `FLEET_CARD` and `FLEET_BOARD`.
- **Verification:** `cargo test -p fleet-cli board`, `make lint`.
- **Done when:** `--blocks` prints two cards, `--clear-agent` sends a nested `None`, and the refusal
  fires with no socket connection.

### P5-T04 — The five run verbs
- **Intent:** Start, stop, list, attach and wait, with the exit codes an orchestrator scripts
  against.
- **Touches:** `commands/board.rs`.
- **Steps:**
  - `card run <key>` → `card_run_start`; prints the new run's id and thread. `card cancel <key>` →
    `card_run_cancel`; the `{KEY} has no live run` refusal is the daemon's.
  - `card runs <key>`: one tab-separated line per run — `id  column  state  provider  model  effort
    duration  tokens  cost  thread` — with `—` for anything unknown, in the shape of
    `human::subagents` (`human.rs:200-218`). State is the outcome word for a terminal run and the
    live delegation's word for a live one.
  - `card attach <key>`: print the live run's thread id, else the newest run's; refuse `{KEY} has no
    run` when the card has none.
  - `card wait <key> [--timeout S]`: `card_run_wait(card, S * 1000)`; exit 0 when the newest run is
    terminal, exit 2 when it is still live or none started within the timeout, through
    `CommandOutput::with_exit_code` (`commands.rs:57`) exactly as `subagents.rs:229-260` does.
  - `--json` for all five prints `BoardCardEnvelope` (the runs ride on the card).
- **Verification:** `cargo test -p fleet-cli board`, `make lint`.
- **Done when:** `wait` returns 0 and 2 in their own tests and no verb panics on a card with no
  runs.

### P5-T05 — Rendering and envelopes
- **Intent:** What a human reads and what a script parses.
- **Touches:** `human.rs`, `envelope.rs`, `commands.rs`.
- **Steps:**
  - `human::board` card row (`:466-471`): `● working` for a live run, `! needs you` when
    `ops::query::attention`, `… pending` for a pending run — before the priority word; ` ⊘ n` after
    the title when `ops::query::blocked` answers `Some`.
  - Column header (`:460-464`) becomes `{name} ({n}) ⚡` when the status carries an action; the board
    header line gains ` · {live+pending}/{max_live_runs} working` and ` · {n} needs you` while
    non-zero.
  - `human::boards` (`:356-394`): `WORKING` and `NEEDS YOU` after `CONFLICTS` in the header
    (`:357-367`) and in `columns()` (`:405`), fed by `BoardSummary.working_count`/`attention_count`.
  - `human::board_card` (`:819`) gains `Agent` (resolved provider/model/effort, each `(column
    default)` when inherited), `Blocked by`, `Blocks` and `Runs` (the `card runs` line).
  - `envelope.rs`: `BoardEnvelope` gains `live_runs` serialised as `liveRuns`; every `columns` verb
    with `--json` prints the `BoardEnvelope` (the columns are `board.statuses`), no new envelope.
  - `human::subagents` (`:200-218`) and `human::subagent_status` (`:229-243`) gain a ninth trailing
    field `caller`: `thread {id}` or `card {KEY}` (the key comes from `Delegation.caller` plus the
    board's prefix, resolved through one `get_board` when the caller is a card); update the `list`
    line in `docs/NATIVE-AGENTS.md` §15.4 (`:1992-1996`) in the same commit.
  - `command_requests_json` (`:263-300`) learns the `Columns` arm and the five card verbs so
    `--json` is honoured before the command runs.
- **Verification:** `cargo test -p fleet-cli human`, `cargo test -p fleet-cli
  board::tests::parsing`, `make lint`.
- **Done when:** The existing row assertion (`tests.rs:588`) still passes for a card with no run,
  and each new mark has its own assertion.

### P5-T06 — Socket tests and the help test
- **Intent:** Assert the exact request every new flag produces.
- **Touches:** `commands/board/tests.rs`, new `commands/board/columns/tests.rs`.
- **Steps:**
  - In `mod orchestration`, one test per new shape through `run(arguments, steps)` (`:1347-1352`):
    each columns verb's `UpdateBoard { patch.statuses }`; `remove --move-cards-to` as `MoveCard`s
    then one `UpdateBoard`; `MoveCard { cancel_run: true }`; `CardRunStart`; `CardRunCancel`;
    `CardRunWait { timeout_ms }`; `CreateCard` carrying `agent` and `blocked_by`; the `--blocks`
    follow-up `UpdateCard`; `UpdateCard` with `agent: Some(None)`; `UpdateBoard` with
    `settings.max_live_runs`.
  - A test that the self-move refusal exits 1 with the sentence and sends **no** request.
  - Extend `board_help_lists_the_new_commands_and_their_flags` (`:1203`) to name `columns`, its six
    verbs, the five run verbs and the new card flags.
- **Verification:** `cargo test -p fleet-cli`, `make lint`.
- **Done when:** Every new request shape is asserted byte-for-byte against a scripted daemon.

### P5-T07 — The smoke script, the Make target, the skills and the doc
- **Intent:** One command that proves the whole feature headless, and two skills an agent can
  follow.
- **Touches:** new `scripts/board-workflow-smoke.sh`, `Makefile`, both skills, `docs/BOARD.md`.
- **Steps:**
  - The script: `set -euo pipefail`; a private `FLEET_HOME=$(mktemp -d)` and `FLEET_DAEMON` pointing
    at the built `fleetd`; a `bin/` first on `PATH` holding a `codex` shim in the shape of
    `agent/launcher.rs:57-87` — answer any `--version` argument with the provider's version line,
    and otherwise `exec fleet-harness agent --provider codex --transcript <child transcript>`; the
    transcript is a `docs/TESTING-HARNESS.md` §5 document whose `{"type":"shell"}` step writes a
    report and runs `fleet subagent complete --result-file`, ending with
    `{"type":"end_turn","status":"completed"}`.
  - Then: `fleet board columns preset workflow`; four `card new` in Todo with `--blocked-by`
    building A→B, A→C, B→D, C→D; one `card move … ready` per card; `fleet board card wait <last>
    --timeout 300` and assert exit 0; `fleet board show --json` and assert every card sits in Done.
    Trap to stop the daemon and delete the home on exit; echo the daemon log path on failure.
  - `Makefile`: `smoke-workflow: build` running the script with `FLEET`, `FLEETD` and `HARNESS`
    exported, plus the name in `.PHONY` (`:36`).
  - `fleet-board-planning`: a "Building a chain" section — create in Todo, write every link, then
    one `card move` per card into Ready (a card that enters Ready before its links are written would
    start at once), watch with `card wait` or `board show`, and never move a card with a live run;
    `references/commands.md` gains the columns family, the five run verbs, the new field flags,
    `liveRuns` and the `0`/`2` exit codes.
  - `fleet-subagent-cli`: `FLEET_CARD` and `FLEET_BOARD` in the environment table (`:8-14`) and the
    self-move rule with its sentence.
  - `docs/BOARD.md` §6: the CLI fence (`:679-695`) gains every new verb and flag, and the notes
    below it gain the exit codes and the self-move refusal.
- **Verification:** `make smoke-workflow`, `make test`, `make lint`.
- **Done when:** The script passes twice in a row from a clean checkout and leaves no daemon or
  temporary home behind.

## Verification

```sh
make lint
cargo test -p fleet-cli
make smoke-workflow
make test
```

## Definition of done

- [ ] Every task above is `[x]` in the tracker.
- [ ] `make lint` is clean; `cargo test -p fleet-cli` and `make test` pass.
- [ ] `make smoke-workflow` drives four cards to Done and `card wait` exits 0.
- [ ] Every new request shape has a socket test and the help test names every new verb and flag.
- [ ] `docs/BOARD.md` §6 and both skills match the shipped flags.
- [ ] The tracker reflects reality; follow-ups are captured.

## Risks and rollback

- **A columns verb clobbers a concurrent edit.** The whole vector is sent; last writer wins, as
  `board set` already does. Documented in §6 rather than solved.
- **The smoke script is the only proof of the chain.** Keep it hermetic — a private home, a private
  `PATH`, no network — or it becomes the flaky target everyone skips.
- **Exit code 2 is load-bearing** for orchestrators; a test asserts it directly rather than through
  a message.
- **An older daemon** answers the three run requests with the client's capability refusal; the CLI
  prints it and exits 1, which a test covers.
- **Rollback.** Revert the phase PR. The daemon keeps every behaviour; only the CLI surface
  disappears.

## Contract resolutions

Resolved in the contracts file on 2026-09-20; the contracts wording wins over any earlier phrasing above.

1. The self-move refusal compares the raw `<key>` argument to `FLEET_CARD` case-insensitively when `FLEET_DELEGATION` is set; it is advisory (contracts §4).
2. `card run` prints `run {id} started, thread {thread}` then the card; `card cancel` prints the card; `card attach` prints the thread id alone (contracts §4).
3. `fleet subagent list` and `status` gain a ninth trailing field `caller` (`thread {id}` or `card {KEY}`) in this phase, with the `docs/NATIVE-AGENTS.md` §15.4 list line updated; add it to P5-T05 (contracts §4).
4. There is no `BoardColumnsEnvelope`: every `columns` verb with `--json` prints the `BoardEnvelope`, whose `board.statuses` are the columns (contracts §4).
