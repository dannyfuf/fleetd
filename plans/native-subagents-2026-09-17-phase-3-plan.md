# Native subagents, phase 3: delegation service and the `fleet subagent` CLI — Plan
> Tracker: ./native-subagents-2026-09-17-phase-3-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

Make delegation work end to end from a shell. A native thread runs `fleet subagent run`, the daemon
creates a child thread of the requested provider with a per-delegation secret in its environment,
appends a delegation row to the caller's transcript, sends the child a brief with a completion
footer, and answers at once. The child reports with `fleet subagent complete`. The daemon decides
"done" as: the report arrived with the right token, the child's turn then settled `Completed`, no
gate is open, and no background task is live. It nudges a child that settles without reporting, at
most twice, then ends it as `Incomplete`. Every ending, including failure and cancellation, is
delivered exactly once into the caller as a new user message with `origin: Delegation`, when the
caller is idle, or by steer if spawned `--eager`. All transitions happen inside the store transaction
that commits the child's event, with follow-up actions in an outbox a worker drains on wake and on
daemon start. At the end of this phase the `agent.delegation` capability is advertised.

## Sizing call

**Phased, phase 3 of 7.** See ./native-subagents-2026-09-17-roadmap.md. This is the largest and
riskiest phase: a new daemon service with a transactional half and a worker half, seven manager
touch points, six wire verbs, and a new CLI noun. It stays one phase because none of its pieces is
useful alone, and one pull request because the completion rules must be reviewed as a whole. The
tasks are ordered so the service is written against the fake harness first and the CLI last, per the
design doc.

## Repository context

- Rust workspace; this phase touches `fleet-daemon` and `fleet-cli`, plus the one-line capability
  advertisement in `fleet-proto`.
- Lint: `make lint`. Test: `make test`; targeted `cargo test -p fleet-daemon delegation`,
  `cargo test -p fleet-cli`. Live harness tests: `cargo test -p fleet-daemon --features real-agents`
  (needs `claude` and `codex` on `PATH`; not part of `make test`). After daemon changes:
  `make restart`.
- Manager: `crates/fleet-daemon/src/services/agents/manager.rs` and `manager/` (`commands.rs`
  has `create`, `send` at line 271, `interrupt`, `stop`; `apply.rs` is the only place a `Seq` is
  minted, and `apply_event` at line 64 requires the thread's serialized-operation guard;
  `controls.rs`, `hydrate.rs`, `checkpoints.rs`, `window.rs`, `mirror.rs`).
- Store: writer thread in `store/writer.rs`; `project.rs` has `append_event` (line 86) and
  `project_event` (line 112), which run inside the writer's transaction; `store/delegations.rs`
  from phase 2 has every delegation statement, each taking `&Transaction`.
- Harness start: `crates/fleet-daemon/src/agents/claude/mod.rs:127` and
  `agents/codex/mod.rs:146` build the child's environment `overrides` and set `FLEET_SESSION`
  and `FLEET_TERMINAL`. The start request type is in `agents/harness/mod.rs`.
- Binaries probe: `AgentBinaries` in `crates/fleet-core/src/config.rs:165`, used by
  `services/doctor.rs:414` and by the manager's start path (find with `grep -rn agent_binaries`).
- Dispatch: `crates/fleet-daemon/src/services/dispatch.rs`, agent arms from line 114; the service
  handle is constructed in `services/composition.rs` (manager at line 72).
- Bus: `crates/fleet-daemon/src/server/broadcast.rs` `BroadcastBus`, bounded, may lag; UI only.
- Fake harness for tests: `crates/fleet-daemon/src/services/agents/manager/tests/mod.rs`
  (scripted provider), tests in `tests/lifecycle.rs`, `tests/restart.rs`, `tests/controls.rs`.
  Other fakes: `crates/fleet-daemon/src/testing/fakes.rs` (`FixedClock` for paused time).
- CLI: `crates/fleet-cli/src/args.rs` (clap derive; `AgentCommand` at line 344 is the pattern;
  `FLEET_SESSION` is already read for `--session` defaults), `commands/agents.rs` for the request
  and output pattern, `envelope.rs` for JSON output, `human.rs` for human output,
  `commands/tests.rs`.
- Client timeouts: `crates/fleet-client/src/connection.rs` (phase 2 set `DelegationWait`).
- Skills: `rust-async-background-work` (worker, wake channel, locks), `rust-ipc-protocol`
  (dispatch, timeouts), `rust-workspace-architecture` (new module, log lines, error types),
  `rust-gpui-testing` (tokio tests, paused clocks), `zed-quality-review` at the end.
- Docs: `docs/NATIVE-AGENTS.md` (§6 write path, §13, new §15), `docs/research/agents-contracts.md`
  (`fleet-cli` section).

## Assumptions

- **Transition half placement.** The transition half is a function in the store layer,
  `store/delegations.rs::transition(&Transaction, thread, &SeqEvent, &ThreadProjection) ->
  TransitionOutcome`, called from `project_event` for every event of a thread that is a delegation
  child *or* a delegation caller. It takes no lock, calls no manager verb, and reports whether it
  inserted an outbox row. The writer pokes the wake channel after `COMMIT` when it did. The design
  doc says "runs inside `apply_event`'s store transaction"; in this codebase that transaction is
  owned by the writer thread inside `project_event`, so that is where it lives.
- **Delivery is recorded in the same transaction as the delivered message.** When `project_event`
  persists an `ItemStarted` whose kind is `UserMessage { origin: Delegation { id } }` on the caller,
  the transition half sets that delegation's `delivery = Delivered { seq, turn }` and marks its
  `deliver` outbox row done. That is what makes delivery exactly once without a second commit.
- **Headline** is updated on `ItemStarted`, `ItemUpdated` (terminal) and `TurnSettled` of the
  child only, never on `ContentDelta`, with the item's title or the first line of the last
  assistant text. One SQL update per item, not per token.
- **Background-task grace** is a named constant, `SETTLE_GRACE`, 30 seconds. A child that settles
  `Completed` with a reported result but a live background task enters `Settling` with a `settle`
  outbox action; the worker waits for the projection to have no background task or the grace to
  pass, then finalizes as `Succeeded` in a new transaction.
- **Token** is 32 random bytes, hex-encoded, from the `rand` crate already in the workspace (check
  `Cargo.toml`; add it under `[workspace.dependencies]` if absent). Stored as SHA-256 hex via the
  `sha2` crate the migrations already use.
- **Result cap** is the item-body budget the transcript already enforces. Use the existing
  constant (`ITEM_BODY_MAX_CHUNK_BYTES` in `fleet-proto::agents`, 256 KiB) and record
  `elided: true` when the CLI or daemon truncates.
- **Mode default** is `PermissionMode::FullAccess` for both providers unless `--mode` is passed.
- **Limits** are constants: `MAX_DEPTH = 3`, `MAX_LIVE_CHILDREN_PER_CALLER = 4`,
  `MAX_LIVE_DELEGATIONS = 8`.
- **`wait` default** is 540 seconds in the CLI, and the CLI caps any `--timeout` at 540 so a
  Claude Bash tool call does not outlive its own ceiling.
- **`ProviderExited` on a child ends the delegation as `Failed`** in this phase. Phase 6 inserts the
  one-resume rule in front of that ending.
- **The nudge text, footer text and delivered-message shape** are taken verbatim from the design
  doc and stored as constants in `delegation/footer.rs` so tests can assert them.

## Out of scope

- Orphan resume with a nudge, undeliverable marking on restart, and cancel propagation to
  grandchildren. Phase 6.
- Any app or kit change. The app already compiles against the shapes (phase 2).
- Remote hosts: `run` refuses a mirrored caller with `ErrorKind::Unsupported`.
- A terminal-based caller via `--caller` from a shell with no `FLEET_SESSION`: the flag is
  accepted and validated exactly like `FLEET_SESSION`, nothing more. The design lists it as
  deferred; accepting the flag costs nothing and is needed by the harness (phase 4).
- Structured JSON results beyond `--json` validation of the file.

## Affected areas

- New `crates/fleet-daemon/src/services/agents/delegation/{mod.rs, run.rs, complete.rs,
  transition.rs, worker.rs, footer.rs, limits.rs, tests/}`.
- `crates/fleet-daemon/src/services/agents/store/{delegations.rs, project.rs, writer.rs}`.
- `crates/fleet-daemon/src/services/agents/manager/{commands.rs, apply.rs, bodies.rs}` and
  `manager.rs` (new public verbs).
- `crates/fleet-daemon/src/agents/harness/mod.rs`, `agents/claude/mod.rs`, `agents/codex/mod.rs`
  (extra environment on start).
- `crates/fleet-daemon/src/services/{composition.rs, dispatch.rs}`.
- `crates/fleet-proto/src/lib.rs` (`AGENT_CAPABILITIES`).
- `crates/fleet-cli/src/{args.rs, commands.rs, commands/subagents.rs, commands/tests.rs,
  human.rs, envelope.rs}`.
- `docs/NATIVE-AGENTS.md`, `docs/research/agents-contracts.md`.

## Tasks

### P3-T01 — Give the manager the four verbs the service needs
- **Intent:** Add the manager-side seams so the service never reaches into manager internals.
- **Touches:** `crates/fleet-daemon/src/services/agents/manager/commands.rs`, `manager/apply.rs`,
  `manager.rs`, `crates/fleet-daemon/src/agents/harness/mod.rs`, `agents/claude/mod.rs`,
  `agents/codex/mod.rs`, `manager/tests/lifecycle.rs`.
- **Steps:**
  - `create` gains an options struct (or builder) with `parent: Option<ThreadId>`,
    `delegation: Option<DelegationId>`, `title: Option<String>`, `extra_env: Vec<(String, String)>`,
    on top of worktree, provider, mode and model. The start request carries `extra_env` and both
    adapters insert it into `overrides` after `FLEET_SESSION` and `FLEET_TERMINAL`. The existing
    dispatch call site passes the defaults.
  - `running_turn(thread) -> Result<Option<TurnId>>`: takes the operation lock, reads the projected
    running turn (the same expression `send` uses at line 285), releases it.
  - `append_item(thread, turn, ItemKind) -> Result<ItemId>`: takes the operation lock and appends
    an `ItemStarted` through `apply_event` under the named turn. Refuses if that turn is not running.
    This is how the `ItemKind::Delegation` row lands in the caller's transcript.
  - `send` already accepts `UserInput.origin` (phase 2); confirm the origin survives into the
    recorded `UserMessage` item on both the announced-turn path and the steer path.
  - Tests: `extra_env` reaches the scripted harness's environment; `append_item` outside a running
    turn is refused; `running_turn` answers `None` when idle.
- **Verification:** `cargo test -p fleet-daemon manager`, `make lint`.
- **Done when:** The three new verbs exist with tests and both adapters forward `extra_env`.

### P3-T02 — The transition half inside the store transaction
- **Intent:** Move a delegation's status, result and delivery in the same commit as the child or
  caller event that caused it, and queue every follow-up in the outbox.
- **Touches:** `crates/fleet-daemon/src/services/agents/store/delegations.rs`, `store/project.rs`,
  `store/writer.rs`, `store/tests.rs`, new `services/agents/delegation/transition.rs` (pure rules,
  no SQL) and `delegation/footer.rs` (constants).
- **Steps:**
  - `transition.rs`: a pure function from `(current Delegation, child event, child projection
    facts)` to `(next status, result change, outbox actions, headline change)`. Encode the state
    machine: `SessionConfigured` → `Running`; `GateOpened` → `Blocked`; `GateResolved` → `Running`;
    `TurnSettled { Completed }` with a reported result and no live background task →
    `Succeeded` + `deliver`; with a reported result and a live background task → `Settling` +
    `settle`; without a report and `nudges < 2` → `Settling` + `nudge`; without a report and
    nudges exhausted → `Incomplete` with the last assistant text as result
    (`source: LastAssistantText`) + `deliver`; `TurnSettled { Error | MaxTurns | BudgetExhausted |
    Other }` → `Failed` with the outcome name and last text + `deliver`;
    `TurnAborted { User | SessionStopped }` → `Cancelled` + `deliver`;
    `TurnAborted { ProviderExited }`, `RuntimeError { fatal }`, `SessionExited { expected: false }`
    → `Failed` + `deliver`. Every terminal transition sets `finished`. Unit-test each arm.
  - `store/delegations.rs`: `transition(&tx, thread, &SeqEvent, &ThreadProjection)` loads the
    delegation by child (or by caller for caller events), runs the pure rules, writes the row and
    outbox rows, returns whether to wake. For a **caller** event: `TurnSettled`, `SessionConfigured`
    or `GateResolved` with any `deliver` row open for that caller → wake; an `ItemStarted` of a
    `UserMessage { origin: Delegation { id } }` → set `Delivered { seq, turn }` and mark that
    delegation's `deliver` row done.
  - `project_event` calls it after the item and turn projections for the same event. The writer
    pokes a `tokio::sync::Notify` (or an `mpsc` of unit) after `COMMIT` when asked. The publish of
    `Event::DelegationChanged` on the bus happens after commit too, from the same place the summary
    is published.
  - The last assistant text is captured at every child `TurnSettled` into `result` with
    `source: LastAssistantText` unless a `Reported` result already exists.
  - Store tests through the real writer: each state-machine arm, delivery recorded on the
    caller's item, and the wake flag.
- **Verification:** `cargo test -p fleet-daemon delegation`, `cargo test -p fleet-daemon store`,
  `make lint`.
- **Done when:** Every arm of the state machine has a test that goes through a real transaction and
  no manager type is imported by `transition.rs` or `store/delegations.rs`.

### P3-T03 — `run`: validate, mint the token, create the child, seed the transcript
- **Intent:** Implement the one request that creates a delegation, with every refusal named.
- **Touches:** new `services/agents/delegation/{mod.rs, run.rs, limits.rs, footer.rs}`,
  `services/composition.rs`, `services/dispatch.rs`.
- **Steps:**
  - `DelegationService` holds the store handle, a manager handle (worker and request handlers
    only), the bus publisher, the wake channel and `AgentBinaries`. Constructed in
    `composition.rs` beside the manager; dispatch routes the six `Delegation*` requests to it.
  - `run` validates in this order and refuses with `validation`, `conflict`, `not_found` or
    `unsupported` errors whose message names the rule: caller exists; caller is not a mirror;
    caller has a running turn (`running_turn`); caller's own depth (its delegation's depth, or 0)
    is below `MAX_DEPTH`; live children of the caller below the cap; live delegations below the
    daemon cap; provider binary present per `AgentBinaries`; worktree resolves (the caller's by
    default).
  - Mint the token; compose the title `↳ <provider> — <first line of the brief>` unless `--title`;
    create the child with `parent`, `delegation`, `FLEET_DELEGATION=<id>` and
    `FLEET_DELEGATION_TOKEN=<token>`, mode `FullAccess` unless given, model if given.
  - Insert the delegation row (`Starting`, `Pending`, token hash) and append the
    `ItemKind::Delegation` row to the caller under its running turn via `append_item`; store that
    `ItemId` as `caller_item`. Order: delegation row first, then item, then the child's first
    message, so a failure between steps leaves a row a later `list` can explain.
  - Compose the first message: the brief, a blank line, `The caller expects: <expectation>`, then
    the footer constant with the delegation id substituted. Send it with `send`.
  - Answer `DelegationStarted(delegation)`. Include a `warning: Option<String>` on the response
    (additive, skipped when `None`) when the child's worktree equals the caller's, with the text:
    "the child edits the caller's worktree; end your turn before it works, or pass --worktree".
  - Harness start failure: the child's `SessionExited { expected: false }` or `RuntimeError`
    during `Starting` reaches the transition half as `Failed` + `deliver`, so the caller is told.
- **Verification:** `cargo test -p fleet-daemon delegation::run`, `make lint`.
- **Done when:** A tokio test with the scripted harness runs a delegation from a caller inside a
  turn and observes the child's environment, the caller's new item, and the child's first message.

### P3-T04 — `complete`: verify the token, record the result, finish if already settled
- **Intent:** Accept the child's report exactly once and trust nothing but the token.
- **Touches:** `services/agents/delegation/complete.rs`, `store/delegations.rs`.
- **Steps:**
  - Load by id; refuse `not_found`. Compare `sha256(token)` to the stored hash in constant time;
    refuse `validation("delegation token does not match")`. Refuse when `child != delegation.child`
    ("this delegation belongs to another thread"). Refuse when the delegation is terminal.
  - Idempotence: a second `complete` with byte-identical text is a success that changes nothing; a
    second with different text is refused with a message naming the first report's time.
  - Truncate the text to the cap, set `elided`. `blocked: true` stores the report and moves the
    delegation to `Blocked` with `status_payload = "reported blocked"`; the transition half ends it
    as `Failed` when the child's turn settles.
  - Store the result as `Reported` in one transaction. If the delegation is already `Settling`
    with no live background task (the child settled before the report arrived, which can happen
    when the harness's `result` frame beats the CLI), finish it now: `Succeeded` + `deliver`.
  - Publish `DelegationChanged`. Answer `Delegation(delegation)`.
- **Verification:** `cargo test -p fleet-daemon delegation::complete`, `make lint`.
- **Done when:** Wrong token, wrong child, second-different-report and second-identical-report each
  have a test, and complete-then-settle and settle-then-complete both end `Succeeded`.

### P3-T05 — The worker: drain the outbox on wake and on start
- **Intent:** Perform every follow-up action outside any lock, one caller at a time, and never lose
  one across a restart.
- **Touches:** `services/agents/delegation/worker.rs`, `delegation/mod.rs`,
  `services/composition.rs`.
- **Steps:**
  - One tokio task, started in `composition.rs`, owning the receive side of the wake channel and a
    `CancellationToken`. Loop: drain, then wait for wake or cancel. Drain once at start before
    serving requests.
  - Drain reads undone outbox rows in id order, groups by caller, and handles one row per caller
    per pass so two children of one caller deliver as two turns.
  - `nudge`: `send` the nudge constant to the child; the store bumps `nudges` in the same
    transaction that records the message (through the transition half seeing the child's
    `ItemStarted`), or, simpler and acceptable, in a small transaction right before the send. Mark
    the row done.
  - `settle`: if the child's projection has no live background task, finalize `Succeeded` +
    `deliver`; else if `created + SETTLE_GRACE` has passed, finalize the same way; else leave the
    row open and re-check on the next wake (the child's terminal `ItemUpdated` wakes it).
  - `deliver`: read the caller's record and summary. Compose the message: first line
    `[fleet subagent <id> finished: succeeded|incomplete|failed|cancelled]`, second line
    `provider: <p>, thread: <child>, duration: <mm>m <ss>s, files changed: <n>`, blank line, the
    report text, and a trailing `(report elided at <n> bytes)` line when elided. Then decide:
    caller `Ready` and idle → `send` with `origin: Delegation { id }` (delivery is then recorded by
    P3-T02's caller-item rule); running a turn and not `eager` → leave the row open (the caller's
    `TurnSettled` wakes it); running and `eager` → `send` (the harness steers); stopped with
    `StopCause::ProviderExit` → `send` (which resumes by cursor); stopped with
    `StopCause::User` → leave the row open (the caller's `SessionConfigured` wakes it); stopped
    with no resume cursor, or record missing → `Undeliverable { reason }` and mark the row done;
    caller blocked on its own gate → leave the row open (its `GateResolved` wakes it).
  - A `send` error that is a `conflict` (the turn started between the read and the send) leaves the
    row open. Any other error is logged with the delegation id and the row stays open; the worker
    retries on the next wake and once per minute via a ticker so a transient failure heals.
  - Log one `tracing::info!` per action with `delegation`, `action`, `caller`, `child` fields.
- **Verification:** `cargo test -p fleet-daemon delegation::worker`, `make lint`.
- **Done when:** The idle, running-then-idle, eager, provider-exit-resume, user-stop-hold and
  no-cursor-undeliverable cases each have a tokio test with a paused clock.

### P3-T06 — `wait`, `status`, `list`, `cancel`, and advertise the capability
- **Intent:** Complete the request set and switch the feature on for clients.
- **Touches:** `services/agents/delegation/mod.rs`, `services/dispatch.rs`,
  `crates/fleet-proto/src/lib.rs`.
- **Steps:**
  - `get` and `list` read the store; `list` filters by caller when given, newest first.
  - `cancel`: refuse when terminal; `interrupt` then `stop` the child through the manager; the
    resulting `TurnAborted { SessionStopped }` reaches the transition half as `Cancelled` +
    `deliver`. A Stop from the UI or `fleet agent stop` on a child takes the same path with no
    extra code. Grandchildren are phase 6.
  - `wait`: subscribe to the bus for `DelegationChanged` with that id, then read the row (read
    after subscribe, so nothing is missed), and return at terminal or when `timeout_ms` elapses
    with the current record. Bus lag is tolerable here because the store read is the truth and a
    lagged subscriber re-reads.
  - Add `AGENT_DELEGATION_CAPABILITY` to `AGENT_CAPABILITIES` in the same commit that routes
    the six requests. Gate `Event::DelegationChanged` emission on the peer's capability.
- **Verification:** `cargo test -p fleet-daemon delegation`, `cargo test -p fleet-proto`,
  `make lint`.
- **Done when:** All six requests dispatch, `wait` returns on the terminal event, and a fresh
  `Hello` from `fleet-client` lists `agent.delegation`.

### P3-T07 — The `fleet subagent` CLI noun
- **Intent:** Give both harnesses the one spawn surface they already have.
- **Touches:** `crates/fleet-cli/src/args.rs`, `commands.rs`, new `commands/subagents.rs`,
  `commands/tests.rs`, `human.rs`, `envelope.rs`.
- **Steps:**
  - `fleet subagent <run|complete|wait|status|list|cancel>` as a new top-level noun beside
    `agent`. Every verb supports the existing `--json` envelope.
  - `run --provider <claude|codex> (--brief-file F | stdin) --expect <text> [--worktree W]
    [--mode M] [--model M] [--title T] [--eager] [--caller <thread>]`. The caller is `--caller`,
    else `FLEET_SESSION`, else a refusal that says which to set. Prints
    `delegation <id> started, child thread <thread>` and the warning line when the response has
    one. Exit 0.
  - `complete (--result-file F | stdin) [--blocked] [--json-result] [<id>]`. The id is the argument,
    else `FLEET_DELEGATION`; the child is `FLEET_SESSION`; the token is `FLEET_DELEGATION_TOKEN`.
    Reads the file at once, refuses a missing or unreadable file with the path in the message,
    validates JSON when `--json-result`, truncates at the cap and says so on stderr, sends the
    content never the path. Prints `reported` on success.
  - `wait <id> [--timeout S]`: default and ceiling 540. Prints the same text the caller would
    receive; exit 0 when terminal, exit 2 when the timeout passed with the status so far.
  - `status <id>` and `list [--caller T]`: one line per delegation: id, status, provider, child,
    age, delivery.
  - `cancel <id>`: prints `cancelled`.
  - Tests in `commands/tests.rs`: argument parsing for each verb, the env fallbacks, the refusal
    when no caller can be found, and the truncation notice.
- **Verification:** `cargo test -p fleet-cli`, `make lint`.
- **Done when:** Every verb parses, calls the matching request, and prints its human and JSON
  forms.

### P3-T08 — The tokio suite the design asks for, plus one live test per harness
- **Intent:** Prove the completion rules without a real harness, then prove each real harness once.
- **Touches:** `services/agents/delegation/tests/{run.rs, complete.rs, endings.rs, delivery.rs,
  limits.rs, live.rs}`, `crates/fleet-daemon/tests/` if the live tests belong with the other
  `real-agents` tests.
- **Steps:**
  - With the scripted harness and a paused clock: complete-then-settle succeeds; settle-without-
    complete nudges once, twice, then `Incomplete` with the last text; a failed outcome delivers
    `Failed`; idle delivery starts a new caller turn with `origin: Delegation`; eager steers a
    running caller; a provider exit on the caller resumes it and delivers; a user stop holds
    delivery until `SessionConfigured`; wrong token refused; grandchild refused against its
    parent's delegation; depth 3, fifth child and ninth delegation refused with the limit in the
    message; two children finishing together deliver as two turns; a Stop on the child delivers
    `Cancelled`.
  - `live.rs` behind `#[cfg(feature = "real-agents")]`: one test per provider with the brief
    "write hello to /tmp/<random>/hello.txt and report the path", asserting `Succeeded` and a
    `Reported` result within a generous timeout. Document in the test how to run it.
- **Verification:** `cargo test -p fleet-daemon delegation`, then
  `cargo test -p fleet-daemon --features real-agents delegation::live` on a machine with both
  binaries.
- **Done when:** Every bullet has a passing test and both live tests pass once before the PR opens.

### P3-T09 — `docs/NATIVE-AGENTS.md` §15 and the CLI contract
- **Intent:** Make the specification exist before the app work starts.
- **Touches:** `docs/NATIVE-AGENTS.md`, `docs/research/agents-contracts.md`.
- **Steps:**
  - New §15 "Delegations": the two-things model (thread plus record), the state machine, the
    three-part definition of done, the nudge rule, the endings table, the delivery rules by caller
    state, exactly-once via the outbox and the caller-item rule, the limits, the token, and the
    verbatim footer, nudge and message texts. Note that recovery and the UI are owed to later
    phases, in the §13 style of naming what is not done.
  - §6 gains one sentence: `project_event` also runs the delegation transition for a child or
    caller event. §13 row 9 becomes "service and CLI served; UI, recovery owed".
  - `agents-contracts.md` `fleet-cli` section lists the six verbs and their environment inputs;
    the daemon section lists `DelegationService` and the manager's three new verbs.
- **Verification:** `git diff docs/` read beside the code.
- **Done when:** §15 states every rule the tests in P3-T08 assert, and nothing more.

## Verification

```sh
make lint
cargo test -p fleet-daemon delegation
cargo test -p fleet-cli
make test
make restart
cargo test -p fleet-daemon --features real-agents delegation::live   # once, with both binaries on PATH
```

A manual smoke after `make restart`: open a Claude tab, ask it to run
`fleet subagent run --provider codex --expect "the file path" --brief-file <a brief that says
"write hello to a temp file">`, end the turn, and watch the result arrive as a new turn.

## Definition of done

- [ ] Every task above is `[x]` in the tracker.
- [ ] `make lint` is clean.
- [ ] `cargo check --workspace --all-targets` is clean.
- [ ] `make test` passes.
- [ ] Both live tests passed once and the tracker notes when.
- [ ] `agent.delegation` is advertised and gates the event.
- [ ] `docs/NATIVE-AGENTS.md` §6, §13, §15 and `agents-contracts.md` match the code.
- [ ] The tracker reflects reality; follow-ups are captured.

## Risks and rollback

- **Deadlock between the writer thread and the manager.** The transition half must never call the
  manager, and the worker must never hold a manager lock across a store call. The compile-time
  guard is that `store/delegations.rs` and `transition.rs` import nothing from `manager`. Review
  that import list explicitly.
- **The Claude `result` frame beats the CLI's `complete`.** Handled by P3-T04's settle-then-complete
  branch; it has a test. If a real run shows a third ordering, add it to `endings.rs` before fixing.
- **A steer into Claude aborts its generation.** That is why idle delivery is the default; `--eager`
  is documented as "you will be interrupted".
- **Bus lag on `wait`.** `wait` re-reads the store after every event and on timeout; a lagged
  receiver reconnects and re-reads.
- **Rollback.** Revert the phase PR. Delegation rows already in the database are ignored by an
  older daemon at slot 3 (the tables stay, unread). A daemon from before phase 2 refuses the
  database; that is the phase-2 rollback note.
