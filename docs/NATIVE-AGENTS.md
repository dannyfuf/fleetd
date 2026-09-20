# Native agents

Authority over: how Fleet runs Claude Code and Codex as structured sessions, the provider-neutral
event model, the thread state machine, the transcript's row vocabulary, the decision surfaces, the
control cluster, and the SQLite store that makes all of it feel instant. `ARCHITECTURE.md`
("Native agent sessions"), `APP-CONTRACTS.md` (§3, §4, §5), `UX-SPEC.md` (§3.6.0), `KEYMAP.md`
("Native agent thread"), `DESIGN-SYSTEM.md` (§6.6), `REMOTE-MACHINES.md` (§7) and ADRs
`decisions/0010-native-agents.md`, `0013-sqlite-agent-transcripts.md`,
`0014-drop-opencode-add-codex.md` carry the seams that touch them; where one of those disagrees
with this file about its own surface, that file wins.

This document is the design **and** what shipped. §13 is the status table, and every "owed" line
in it is a thing this build does not do, named there rather than softened in the section that
specifies it; `TODO.md` at the repo root carries the same list with what each one costs. The
previous revision described a Claude Code + OpenCode implementation, which has been replaced —
OpenCode is deleted (ADR 0014).

Sources this was distilled from:

- **The installed harnesses, captured live** on 2026-09-17: `claude 2.1.275` and
  `codex-cli 0.154.0`. `research/harness-protocols.md` and
  `research/harness-codex-app-server.md` are the wire references, and they outrank any SDK
  typing or vendored schema. Regenerate the Codex protocol with
  `codex app-server generate-json-schema --out <dir> --experimental`.
- **t3code** (`pingdotgg/t3code` @ `d29c56a`), a desktop control surface for the same two
  harnesses. Cited throughout as `apps/server/src/…` and `apps/web/src/…`. It is prior art with
  real production bugs baked into its comments — it is evidence, not a design authority, and
  where it is wrong this document says so and diverges.
- The **Fleet Redesign** canvas, artboards *Agent tab — working*, *Agent tab — needs you*,
  *Agent thread — message vocabulary*, *Signals*, *Workspace*, *Terminal chrome*.
- The current Fleet code, which is **vocabulary only**.

## 1. Goal

A native GPUI conversation that Fleet understands: it knows when a turn is running, when the
agent is blocked on a permission, a question or a plan, when a turn finished, and when a process
died. The terminal path stays as an explicit fallback on `^s F`.

Two harnesses, and only two: **Claude Code** and **Codex**. Every table in this document carries
a Claude column and a Codex column. Where a row would read "the provider", it is written out
twice instead, because the two disagree more often than they agree.

Remote worktrees use the same experience through daemon federation. The owning daemon runs the
harness and owns the transcript; the local daemon routes, and — new in this revision — keeps a
durable read-through **mirror** so a remote thread paints in one frame (§9.3).

Non-goals: ACP agents, cross-host thread migration, a thread-list sidebar, voice, a full editor
as the composer, and Codex's realtime/audio surface.

## 2. Product shape

The canvas fixes these decisions; do not relitigate them in code.

- **Agents are workspace tabs, not a separate screen.** A Claude or Codex session is a numbered
  tab in the same strip as terminals: `[2] claude — rounding fix`, `[6] codex — tz shifts`.
  `^s a` starts Claude, `^s A` starts Codex, `^s 1–9` selects, `^s x` closes the *tab*, not the
  thread. §8 keeps every thread browsable. The installation remembers the close, the daemon
  persists it beside the read cursor, and the `AGENTS` picker reopens it. The marker is daemon-side
  because §9.2 permits no app-side on-disk mirror or cache. No thread-list sidebar, no inspector,
  no detached diff pane. Wide windows leave the right side empty on purpose; the content measure
  is 760 px with a 16 px inset.
  A child thread is in the strip only when attached; detaching it changes this window's tab set,
  not the daemon-owned thread or its durable delegation.
- **The pane is a transcript above a docked composer.** The transcript is bottom-anchored. The
  composer is a multi-line input (`❯` glyph, placeholder `Message claude… (@ file · / command ·
  $ skill)`), Enter sends, Shift+Enter inserts a newline, no send button, and a 22 px metadata row:
  `agent mode · claude-sonnet-5 · high · asks before edits` on the left, `context 34% · $0.42 ·
  48m` on the right. Every left-hand segment the harness reports is shown and no segment is
  invented.
- **Tab titles differ per harness, and that is the harness's limit.** Codex publishes
  `thread/name/updated` and it replaces the fallback as soon as it arrives. Claude publishes no
  title on any frame, so a Claude tab keeps the fallback — the first user message, normalised and
  cut at 48 characters — for the life of the thread.
- **One visual vocabulary for both harnesses.** User turns are the only block with a background
  (`bg.panel`, radius 6). Assistant prose sits on the ground with no bubble, avatar or header.
  Each prose block is one shaped text element, so inline marks do not turn paragraphs into flex
  items and an unbroken token wider than the 760 px measure still wraps. GFM tables use equal,
  wrapping columns and an emphasised header separated by one hairline. Fenced code alone keeps
  its line structure in a horizontal scroller; a vertical wheel over it continues scrolling the
  transcript.
  Reasoning is one collapsed muted line. Every tool call is one 30 px row: `state glyph · 60 px
  kind column · one-line summary · right-aligned result`. Completed work folds to
  `worked 22s · 14 steps  ⌄`. A turn ends with one right-aligned footer.
- **Approvals and questions dock above the composer. Only the proposed plan is an in-transcript
  card.** This **reverses** the previous revision of this document, which put all three in the
  transcript as the last item. §6.1 carries the argument; the short form is that a transcript card
  can be scrolled out of the viewport while it still owns the keyboard, which is a modal with the
  chrome removed.
- **Colour is semantic.** Green = alive/fine, amber = needs you or cannot verify, red = broken,
  gray = everything else including progress, blue = where you are and never a state. Only gray
  spinners and the text cursor animate. Attention is a static amber dot or bar.
- **Where state shows.** Tab: gray spinner (running), amber dot (needs you), gray dot (unread
  output), `exited 1` in red (dead). The amber dot is the one mark that survives selection — a
  thread blocked on you is blocked whether or not you are reading it. Session header: `working` or
  `needs you`. Context bar: `3 needs you · 2 working · 1 failed`, which includes the current tab
  and omits every segment whose count is zero.

Exact copy, keys and dimensions are in the canvas; `UX-SPEC.md` §3.6.0 carries the agent tab
section, and `DESIGN-SYSTEM.md` §2.8 carries the fixed dimensions as `components::agent::metrics`
constants.

## 3. Architecture

Fleet's rule holds: the daemon owns truth, clients consume snapshots and events, rendering never
blocks on IO. Agent sessions live in the daemon, survive app restarts like terminals do, and
additionally survive daemon restarts through the persisted transcript and harness resume cursors.

```
fleet-daemon
  services/agents/
    manager.rs          AgentSessionManager: threads, lifecycle, sequencing, attention
    thread.rs           one thread's runtime: serialized operation gate, delta coalescing
    store/              SQLite: schema.rs migrations.rs writer.rs read.rs cursor.rs
  agents/
    harness/            trait Harness, HarnessCapabilities, HarnessError, spawn(), probe, ndjson
    claude/             transport argv frames map session
    codex/              transport wire methods map approvals session
fleet-core
  agents/               ids, AgentEvent, items, gates, state enums, projection (the reducer)
  paths.rs              FleetHome::agents_path/agents_db_path/agents_attachments_path
fleet-proto
  agent requests / responses / events / summaries (additive, capability-gated)
fleet-client
  api/agents.rs         typed commands; api/agents/mirror.rs applies ordered deltas, resync on gap
fleet-app
  screens/agent_thread/ Entity<AgentThreadView>: rows, dock, presentation, pickers
  state/agents.rs       summaries, projections, seen cursors, attention edges
fleet-ui-kit
  components/agent/     TranscriptList, ToolRow, DecisionDock, MetadataRow, format, metrics
fleet-lazygit
  diff_view.rs          the reusable inline DiffView (kept out of the kit; ADR 0010)
```

Ownership boundary: **harness IO produces normalised events; a serialised reducer persists them
and updates projections; views render projections.** GPUI never owns lifecycle truth. Harness wire
types never leave `crates/fleet-daemon/src/agents/` — `fleet-proto` never learns the word
`turn/start`. `fleet-core` stays IO-free, which is why SQLite lives in `fleet-daemon` and nowhere
else (`ARCHITECTURE.md` "crate layering").

Nothing in the new code exceeds ~900 lines. `codex/wire.rs` and `codex/methods.rs` are
**generated and checked in**: hand-transcribing 99 request methods, 72 notifications and 18 item
variants rots inside one Codex release.

### 3.1 The adapter seam

```rust
/// One live harness process bound to one thread. Owned by the daemon's
/// AgentThread task; never shared, never Clone, never reachable from a client.
#[async_trait]
pub trait Harness: Send + 'static {
    fn kind(&self) -> HarnessKind;
    fn capabilities(&self) -> &HarnessCapabilities;
    async fn open(&mut self, req: OpenSession) -> HarnessResult<SessionOpened>;
    async fn submit(&mut self, req: Submit) -> HarnessResult<Submitted>;
    async fn interrupt(&mut self, turn: TurnId, reason: InterruptReason) -> HarnessResult<()>;
    async fn answer_gate(&mut self, gate: GateId, answer: GateAnswer) -> HarnessResult<()>;
    async fn apply_runtime(&mut self, change: RuntimeChange) -> HarnessResult<RuntimeApplied>;
    async fn compact(&mut self) -> HarnessResult<()>;
    async fn shutdown(&mut self, reason: ShutdownReason) -> HarnessResult<()>;
    fn events(&mut self) -> mpsc::Receiver<HarnessEvent>;
}

pub async fn spawn(kind: HarnessKind, cfg: &HarnessConfig, probe: &ProbeCache)
    -> HarnessResult<Box<dyn Harness>>;
```

Construction is a free function because the failure modes differ before a process exists.

On Linux every probe and session child installs `PR_SET_PDEATHSIG(SIGKILL)` and verifies that its
parent did not change across fork/exec; macOS has no parent-death signal and relies on stdin EOF.

**Which binary.** `HarnessConfig.command` comes from `config.agentBinaries.{claude,codex}` — the
executable the daemon `execve`s, with **no shell**. It is deliberately *not* `config.agentCommands`,
which is the shell line a PTY pane types and may legally be a shell function or an alias: `cc` in a
pane is the user's wrapper, `cc` to `execve` is the C compiler. A bare name is resolved against the
login shell's `PATH`; fixed arguments are allowed and tokenized with POSIX quoting.

**The probe checks identity, not just semver.** `<binary> --version` must *name the harness* —
`Claude Code` for Claude, `codex` (case-insensitive) for Codex — before its version is parsed at
all. Without that check any program with a version line passes: `cc --version` answers
`cc (GCC) 16.2.1`, whose first semver token clears every floor Fleet could set, and gcc is
accepted as "Claude Code 16.2.1". A refusal is `HarnessError::Unavailable` whose reason names the
**configured command** and quotes the first line of what it printed
(`` `cc` is not Claude Code: `cc (GCC) 16.2.1 20260810`. ``). A successful probe logs at `info`
with the resolved absolute path and the parsed version — the one line that says which binary a
thread is about to run. `ProbeCache` is keyed by `(kind, command, home)`, so correcting
`agentBinaries` is a cache miss and takes effect on the next create.

**A failed start is loud.** Both adapters keep a bounded tail of the child's stderr (20 lines or
2 KiB, whichever comes first) in a buffer the drain task appends to, so `open` can read it
*synchronously* before the transport — and with it the drain task — is dropped. A child that dies
inside the spawn window reports the configured command, its exit code and that tail:
`` `cc` exited with code 1 during startup: cc: error: unrecognized command-line option
'--output-format' ``. The manager logs one `warn` naming the provider, the command and the
worktree before the typed error goes out on the wire; without it a mis-configured
`agentBinaries` entry left no trace in `fleetd.log` at all.

| Method | Race it closes |
| --- | --- |
| `capabilities` | Offering "Always allow" on a Codex exec approval, which the wire cannot express and which t3code silently downgrades to session scope (`CodexSessionRuntime.ts:2004`) — a button that lies. |
| `open` | A `submit` racing the handshake. `open` returning is the barrier. |
| `submit` | The composer deciding "is this a steer?" from a stale mirror of `active_turn_id`. The *adapter* knows the wire form; the caller must not guess. |
| `interrupt` | A wedged subagent blocking the parent interrupt forever — exactly during the runaway fleet where Stop matters most. |
| `answer_gate` | Answering a gate the harness already withdrew, which on Codex is a protocol error and on Claude a silent no-op. Gates outlive turns, app connections, and can be withdrawn. |
| `apply_runtime` | Applying a model change mid-turn. Neither harness honours it mid-turn; both would silently defer and the UI would lie. Returning `RuntimeApplied` moves the restart decision to the manager, the only thing that knows whether a turn is running. |
| `shutdown` | A pending approval nobody can answer keeping the thread permanently unsettled. |
| `events` | A lossy fan-out dropping a `TurnSettled` and spinning the UI forever. Taken once, so a second consumer cannot exist. |

`HarnessEvent` is `{ observed_at, emitted_at: Option<SystemTime>, event: AgentEvent, raw:
Option<RawRef> }`. `emitted_at` exists because every Codex notification carries a top-level
`emittedAtMs` — the only server-side clock Fleet gets, and what makes ordering measurable across a
remote link. It stops at the adapter bridge, which turns it into `observed_at - emitted_at` and
logs a frame read more than 250 ms after the harness says it emitted one: `SeqEvent` has no field
for it, and a wire field with no reader is worse than a log line that answers the question the
latency budget actually asks (§9.4). `raw` is a **pointer** into the per-thread NDJSON debug log,
never an inline payload: inlining raw frames is what makes a snapshot undecodable at 16 MiB.
`RawRef.method` is populated; `RawRef.offset` stays `None` until that log exists (§13).

**The manager owns the restart, and refuses it mid-turn.** `apply_runtime` reports a
`RestartPlan`; `AgentSessionManager::apply_control` performs it, and only at a turn boundary —
§7's "nothing applies mid-turn" means a change whose cost is a restart is refused while a turn
runs with a typed `Conflict` naming the fields, rather than killing the turn to make a picker
truthful. `session = Starting` is published before the replacement process opens, and a failed
restart publishes `Error` before it returns, so the tab never keeps an optimistic `starting…` a
failure invalidated.

`HarnessCapabilities` carries a `ControlCost` per control (`InPlace | RestartWithResume |
NotSupported`). That field is what makes the UI honest: Claude's model and effort are launch
flags, Codex takes both per turn, so the same "switch to a bigger model" gesture costs a process
restart on one harness and nothing on the other.

`HarnessError::Protocol` carries a `SchemaFingerprint { issue_count, issue_kinds, max_path_depth,
present_fields }` and **never** a payload, a field value, or a string derived from one. Agent
transcripts carry source, credentials and customer data; `tracing::warn!(?payload)` on a decode
failure is an exfiltration bug. Three tests per harness enforce it.

The trait forbids four things: no method returns transcript state (that would give the daemon two
orderings of one truth); no method blocks on a human (`answer_gate` writes and returns); no
adapter holds a GPUI handle or reads config at call time; and **no adapter retries a turn** —
both harnesses retry internally and say so, and a Fleet-level retry would double-charge the user
and duplicate side effects.

### 3.2 Normalised event model (`fleet-core::agents`)

Identities: `ThreadId`, `TurnId`, `ItemId`, `GateId`, plus a per-thread monotonically increasing
`Seq(u64)` stamped by the reducer. The existing `ids.rs` survives unchanged.

Event families: **session** (`SessionConfigured`, `SessionStateChanged`, `SessionActivity`,
`SessionExited`), **turn** (`TurnStarted`, `TurnSettled`, `TurnDiff`, `PlanSteps`), **items**
(`ItemStarted`, `ContentDelta`, `ItemUpdated`, `ItemCompleted`), **gates** (`GateOpened`,
`GateResolved`, `GateWithdrawn`, `PlanProposed`), **observability** (`TokenUsage`, `RateLimits`,
`Compacted`, `Retrying`, `ModelRerouted`, `RuntimeError`, `Notice`, `Unknown`).

`StreamKind` is the channel discriminator on `ContentDelta` and it is where a real divergence
lives: `AssistantText`, `ReasoningSummary { part }`, `ReasoningRaw { part }`, `CommandOutput`,
`PlanText`. Claude fills only `ReasoningSummary { part: 0 }`; Codex fills both reasoning channels
and marks paragraph boundaries. Concatenating across a Codex `summaryPartAdded` produces one
run-on wall of text, so the part index is load-bearing.

`ToolKind` is the design's kind column: `Read | Edit | Write | Bash | Search | Grep | Fetch |
Agent | Todo | Skill | Mcp { server } | Unknown`. It is fed by `tool_name` on Claude and by
`commandActions[]` on Codex (§4.2), degrading to the raw command line on `{type:"unknown"}`.

`ItemStatus` is `InProgress | Completed | Failed | Denied | Stopped`. **`Denied` is first-class**
— a refused tool is not an error, and both harnesses distinguish them
(`CommandExecutionStatus = inProgress|completed|failed|declined`).

### 3.3 Thread state: orthogonal axes, not one enum

```
session:  Starting | Ready | Running | Waiting(reason) | Stopped | Error
turn:     None | Running(TurnId) | Settled(TurnId, outcome)
gates:    Vec<OpenGate>            // permissions, questions, actionable plan
work:     open_items, background_tasks (subagents), retrying
view:     last_seen_seq            // per installation, persisted daemon-side
```

A stopped thread also keeps `stop_cause: User | ProviderExit` on its durable record. Session
state alone says only that the process is stopped; the cause lets recovery and delegation
delivery distinguish an intentional Stop from a provider crash.

The client library creates one UUID at `FleetHome::client_id_path()` on first use and sends it as
`HelloClient.client_id`. Every Fleet window using that home shares the cursor; another installation
has another UUID and cannot clear its mark. A newer in-process cursor remains an immediate local
override until its monotonic `AgentMarkSeen` write is echoed by a later connection. A clean daemon
shutdown's synthetic `SessionStateChanged(Stopped)` is not unread transcript output: the same
writer transaction advances only cursors that were already exactly caught up before that event.

Derived **attention**, in priority order, carried in the thread summary so the tab, the header and
the context-bar counts agree:

| Attention | When | Tab | Header | Context bar |
| --- | --- | --- | --- | --- |
| `NeedsYou(Permission)` | an open permission gate | amber dot | `needs you` | `needs you` |
| `NeedsYou(Question)` | an open **blocking** question gate | amber dot | `needs you` | `needs you` |
| `NeedsYou(Plan)` | a settled plan waiting for a decision | amber dot | `needs you` | `needs you` |
| `Working` | `session == Running` or `turn == Running` or background tasks alive | gray spinner | `working` | `working` |
| `Waiting(UsageLimit)` | a rejected rate-limit window with no allowed overage | gray spinner + countdown | `waiting` | `waiting` |
| `Failed` | turn failed, session error, or unexpected exit | `exited 1` red | `failed` | `failed` |
| `NeedsYou(Finished)` | a turn settled and its `seq` is newer than `last_seen_seq` | amber dot | `needs you` | `needs you` |
| `Unread` | new non-terminal output since `last_seen_seq` | gray dot | — | — |
| `Idle` | otherwise | plain | `idle` | — |

The row order **is** the priority order. A turn that finished and has not been read may not shadow
a session that is running now or one that has died — a killed harness reads `exited 137` in red,
not `needs you`. A **non-blocking** Codex question (`isBlocking: false`) never produces
`NeedsYou`: the turn proceeds past it, and marking the thread blocked would falsely fill the
user's inbox.

Broader worktree states win over agent states: offline > job phase > agent state > session state.

Transition rules, each closing a real race:

1. `Running` is set only by `TurnStarted`. Deltas and items may only land in a known turn and
   item; a terminal event whose turn is not the active turn is dropped and logged.
2. **Only the harness's authoritative primitive settles a turn** (§4.4). Never silence, never a
   stream closing, never process liveness, never a status hint.
3. Before `TurnSettled` is applied, every open item in that turn is closed so the transcript never
   shows a spinner on a finished turn, except a live background item: a subagent can outlive the
   turn that launched it and remains live in the projection until its own terminal event.
4. Gates are independent of turns. A gate closes only on `GateResolved` or `GateWithdrawn`.
5. Stream or process loss is `SessionExited { expected: false }` plus `RuntimeError`, which makes
   the turn `Failed`. Never inferred success. Restart recovery is the explicit exception: it
   follows rule 9 and aborts a resumable orphan as `Interrupted` before leaving it `Stopped`.
6. A user message sent while running is **steering**, dispatched immediately. There is no queue
   and no `QueuedMessage` row — see §7.2.
7. Attention derives from gates, then work, then failure, then fresh completion.
8. **Settlement is sticky.** `Interrupted` never downgrades to `Completed`; `Failed` never
   downgrades. This is what makes "the user pressed Stop and the turn completed anyway" render
   correctly.
9. **A thread whose provider is gone comes back lazily, on the next open or send.** A thread with
   a resume cursor and at least one turn is resumed from that cursor. A thread that never started
   a turn has nothing to resume — the harness never wrote a conversation for its cursor — so it is
   `Stopped` after a daemon restart, not `Error`, and the next open or send **starts it over**
   with a fresh launch. Only a thread with turns and no cursor stays `Error`.

One pure function, ~30 lines and unit-tested, guards the whole class of "the UI says it is still
running / it says it finished but it did not" bugs:

```rust
pub fn should_apply_lifecycle(
    event: LifecycleKind, event_turn: Option<TurnId>,
    active_turn: Option<TurnId>, pending_start: Option<TurnId>) -> bool
```

A stale terminal event for an old turn never ends the current turn; a terminal event with no turn
id while a turn is active is ignored because it cannot be attributed; a `TurnSettled` naming a
turn Fleet never saw start **is** accepted, because it recovers a lost start; a `TurnAborted`
naming an unknown turn is rejected, because a delayed stop must not clobber a newer pending start.
Codex also validates a non-empty string `/turn/id` before accepting a successful `turn/start`
response. If one stdout burst maps `turn/started` and `turn/completed` before the response future
resumes, the cleared pending start is proof that the response has nothing left to adopt or emit.

The PTY fallback reuses only the `AttentionKind` vocabulary and the amber `NeedsYou` tab mark; it
does not imitate this reducer.

## 4. Harness integration

Both adapters follow one rule: **speak the structured protocol, never parse the terminal.** The
wire references are `research/harness-protocols.md` (Claude) and
`research/harness-codex-app-server.md` (Codex), both captured from the installed binaries.

### 4.1 Claude Code

Driven over its own stdio, in Rust. There is no Rust Agent SDK and there will be no Node sidecar:
`fleetd` has no Node dependency and is not acquiring one for a single NDJSON format.

```
claude -p
  --output-format stream-json --input-format stream-json --verbose
  --include-partial-messages
  --permission-mode <acceptEdits|auto|bypassPermissions|manual|dontAsk|plan>
  --permission-prompts host
  --permission-prompt-tool stdio
  --model <id> --effort <low|medium|high|xhigh|max>
  --add-dir <attachments-dir>
  [--session-id <fleet uuid> | --resume <session-id> [--fork-session]]
```

Three flags carry corrections to the previous revision of this document:

- **`--effort` is real in 2.1.275.** The old claim that Claude has no reasoning ladder — and the
  three-segment metadata row sized around it — is dead. Effort is a launch flag and is **not**
  echoed on `system/init`, so Fleet must remember what it launched with.
- **`--permission-prompt-tool stdio` is not optional.** With `--permission-prompts host` alone, a
  tool needing approval produces **no** `control_request`. It produces
  `{"type":"system","subtype":"permission_denied",…}` and the tool is silently refused. Fleet would
  show no card and the user an inexplicable denial.
- **`system/init.permissionMode` is advisory.** `manual` comes back as `"default"`. Fleet keeps
  its own record.

Environment: Fleet builds the child environment explicitly and **strips its own `CLAUDE_*`
inheritance** (`CLAUDE_CODE_SESSION_ID`, `CLAUDE_EFFORT`, `CLAUDE_CODE_ENTRYPOINT`,
`CLAUDE_CODE_MESSAGING_SOCKET`). Inherited, they silently re-point every thread Fleet starts. Per
instance config isolates through `CLAUDE_CONFIG_DIR`, **never by overriding `HOME`** — on macOS
that relocates the login keychain and the CLI reports "Not logged in"
(t3code `ClaudeHome.ts:118-127`). `CLAUDE_CODE_AUTO_CONNECT_IDE=0` and
`CLAUDE_CODE_IDE_SKIP_AUTO_INSTALL=1` are set on the **session** as well as the probe: a session
without them spawns the IDE-discovery process tree the probe's own hygiene exists to avoid, once
per thread, for the life of the daemon. Both are defaults — an explicit `StartRequest.env` entry
still wins.

The binary is `config.agentBinaries.claude`, not `config.agentCommands.claude` (§3.1).

Before the first prompt Fleet sends an `initialize` control request and reads `models`; if that
answer is empty it retries with `list_models`, then falls back to the adapter's four-model static
catalogue with a warning. The live 2.1.275 response identifies a selectable model by `value`, a
label by `displayName`, and its legal effort ladder by `supportedEffortLevels`. Fleet publishes
that normalized catalogue in `SessionConfigured`, so Claude uses the same `^s m` / `^s e`
picker path as Codex. Because `system/init.model` is a resolved id rather than a selectable
`value`, the adapter retains the `value` → `resolvedModel` aliases and publishes the selectable id
again (including `default` and `[1m]` variants); the raw resolved id remains internal for matching
`modelUsage`.

Handshake is `system/init`. Fleet reads six fields: `session_id` (**the resume cursor**, persisted
before any turn), `capabilities` (the version gate), `model`, `permissionMode` (advisory), the
`tools`/`slash_commands`/`skills`/`agents`/`mcp_servers` vocabularies for the composer's `/`, `@`
and `$` menus — **per session, discovered here, never hardcoded** — and `claude_code_version` for
the log.

**`system/init` arrives with the first prompt, and again with every later one** (observed live on
2.1.275: one init per turn). Three consequences, each implemented:

- `open` does not wait for it. The barrier is "the child survived its spawn window", and a child
  that did is accepting input, so the adapter publishes `SessionStateChanged(Ready)` at open and
  `system/init` re-affirms it later. Without this a fresh thread read `starting…` — the working
  spinner — until the user typed, which is indistinguishable from a hang.
- The cursor is durable from `create`: the adapter mints the session id it launches with, hands it
  back as `SessionOpened.resume_cursor`, and the manager writes it into the thread record before
  the harness has said a word. A daemon that goes away between the first prompt and init still
  comes back to the same conversation.
- Repeated inits configure the session **once**: the mapper ignores every init after the first,
  and the cursor keeps following durable frames as before.

`--resume <cursor>` for a session the CLI never wrote a conversation for prints `No conversation
found with session ID`, emits one `result` with `is_error: true` and exits 0. The manager never
asks for that (§3.3 rule 9: a thread with no turn starts over), and the adapter catches the case
anyway: a `--resume` child that dies inside the spawn window with that sentence on stderr is
relaunched **once** with `--session-id <the same cursor>`, so the thread keeps the cursor the store
already holds.

Frames not in the previous revision, all observed live: `system/status` (a truthful spinner
sub-label, never a terminal), `system/thinking_tokens` (a live reasoning-token counter that lets
the collapsed row tick `thinking · 132 tokens` with the body closed), `rate_limit_event` (two
utilisation windows, §4.1.2), and `system/permission_denied` (above — **must render**; dropping it
makes a refusal look like a hang).

Streaming: `stream_event` deltas move the cursor, `assistant` frames backfill completed blocks —
**one frame per content block, several frames sharing one `message.id`**, so the *content block*
is the transcript atom, not the message. `content_block_stop` closes the item but retains its
`(parent_tool_use_id, message.id, block index)` correlation; the later `assistant` snapshot
consumes that correlation, patches the same item only when its text differs, and otherwise emits
nothing. Four further invariants: `signature_delta` is **dropped** (it
closes the thinking block's signature and is not display text); a reused block index within one
turn must mint a **new** item id; the assistant text item closes before the following tool item
opens; and `input_json_delta` is accumulated and re-parsed, emitting `ItemUpdated` **only when the
parsed input's fingerprint changes** — the difference between ~1 and ~30 events per tool call, and
over SSH the difference between a live transcript and a slideshow.

Subagent narration is dropped when `parent_tool_use_id` is set; subagent *tool* blocks are not.
t3code records both halves as live findings: emitting the narration "interleaved N subagents'
narration into the chat"; dropping the input deltas "emptied attributed tool inputs"
(`ClaudeAdapter.ts:2645-2673`).

**Completion authority is the top-level `result`, exactly one per turn.** All three of `subtype`,
`is_error` and `terminal_reason` must be read; a `subtype: "success"` can still carry
`is_error: true`. `modelUsage` and `total_cost_usd` are **cumulative for the process** — take the
latest, never sum. `modelUsage[model].contextWindow` is the context-meter denominator, published
only here. A `result` with no locally-tracked turn emits usage and a tripwire log line and **no
settlement**.

Gates all arrive as `control_request { subtype: "can_use_tool" }`; the tool name is the
discriminator. Fleet intercepts `AskUserQuestion` and `ExitPlanMode` **before** any
permission-mode short circuit, because plan mode depends on questions working even in a fully
permissive mode. `ExitPlanMode` is **always denied** with a stop-and-wait instruction, and the
plan markdown read from `input.plan`.

**Scope rewriting is a safety requirement.** The live suggestion came back
`destination: "localSettings"`. Echoing it verbatim for a *session* choice writes a **permanent**
rule into the user's `.claude/settings.local.json`. Fleet rewrites `destination` to `"session"`
for the session decision and only offers a persistent decision with copy that says so. Where
Claude offers no suggestion at all — common for MCP tools — the session decision falls back to a
whole-tool session allow rule, because degrading it into a one-shot means the card returns on the
next call and the user believes the toggle is broken. t3code's `acceptAlways` falls into its deny
branch (`ClaudeAdapter.ts:4623-4636`) and therefore silently *denies*; Fleet implements all three
scopes.

Interrupt is `control_request { subtype: "interrupt", cancel_queued }`, gated on the declared
capabilities, with a receipt-then-escalate ladder: receipt expected within 2 s (a nicety), the
`result` within 10 s — **the interrupted turn still emits its `result` and that is what settles
it** — then stdin close, SIGTERM, SIGKILL. t3code interrupts by hard session close
(`ClaudeAdapter.ts:5008`), costing a full resume; Fleet owns the wire and does not copy that.

#### 4.1.2 The parked turn

A rate-limit window whose `status == "rejected"` with no allowed overage **parks the turn inside
the CLI: no further frames arrive and no `result` ever lands.** Without a row the thread spins
forever. Fleet emits `SessionStateChanged { Waiting { UsageLimit { window, resets_at } } }` — a
first-class state, not a warning row, so the app ticks a live countdown locally instead of
rendering a string that goes stale — deduplicated per turn per `<window>:<resets_at>`.
`allowed_warning` stays quiet: it still has headroom.

### 4.2 Codex

`codex app-server` is a child process speaking newline-delimited JSON in a **JSON-RPC-*shaped*
protocol that is not JSON-RPC 2.0**. A strict JSON-RPC crate is the wrong tool: there is **no
`jsonrpc` field in either direction**, and `params` is omitted entirely when absent — not `null`,
not `{}`. Classification is structural: `method` + a string-or-number `id` is a request; `method`
with the key `id` **absent** is a notification (`{"method":…,"id":null}` is *not*); a valid id with
`result` or `error` is a response.

Spawn is `codex app-server [user args] [-c key=value]…`. **Fleet never writes `config.toml`** —
all configuration is `-c` on argv, and `CODEX_HOME` selects which file Codex reads, manually
tilde-expanded because `spawn` performs no shell expansion.

Handshake: `initialize` → `{userAgent, codexHome, platformFamily, platformOs}` → `initialized`.
Three things follow. All server-request handlers must be registered **before** `initialize` is
sent, because the server may issue a request as early as its first reply. **There is no protocol
version** — the CLI version is scraped from `userAgent`. And
`capabilities.experimentalApi: true` is what unlocks `item/tool/requestUserInput`.

**`optOutNotificationMethods` is a server-side event filter and Fleet uses it aggressively.**
Cutting an event at the source beats filtering it in the daemon, and it matters most over a remote
link — `rawResponseItem/completed` alone roughly doubles the byte volume of a turn and is pure
debug data. The suppression list (realtime/audio, fs, process, fuzzy search, raw response items,
moderation metadata, Windows sandbox) is a golden-tested constant.

From the codex-cli 0.154.0 schema Fleet uses these methods: `initialize`,
`thread/start`, `thread/resume`, `thread/read`, `thread/items/list`, `turn/start`, `turn/steer`,
`turn/interrupt`, `thread/settings/update`, `thread/compact/start`, `thread/fork`,
`thread/unsubscribe`, `model/list` (**cursor-paginated — loop on `nextCursor`**), `skills/list`,
the two account reads, and the three account controls — `account/login/start`,
`account/login/cancel`, `account/logout`. Each carries a Fleet-side deadline, because the
protocol has none, and each deadline is strictly less than the `fleet-proto` timeout of the
client call that triggered it, so the daemon always answers before the client gives up:
`account/read` is allowed 5 s inside the handshake, and the three controls 10 s each inside the
45 s `AgentAccountLogin` / `AgentAccountLogout` request.

**The account is read, never assumed.** `account/read` runs once right after `initialize`, so the
metadata row is honest in the first frame the tab paints and a signed-out session says so before
the first refused turn instead of after it. A failed or timed-out read is one `warn!` and no
event — Codex's answer to "who am I" is not worth a failed start. `account/updated` carries
`authMode` and `planType` and **never the email**, so it is treated as a bare "something changed"
and answered with a real `account/read`; `account/login/completed` is treated the same way on
success, because it names no account either. Its failure is the one case that is a transcript row
on its own, because the user is standing in front of a browser wondering what happened. A second
`/login` while one is pending cancels the first with `account/login/cancel`: two live callbacks
mean two browser tabs that both claim to be the sign-in, and only one of them can win.

Explicitly **not** used: `fs/*`, `command/exec/*`, `process/*` and `fuzzyFileSearch` (Fleet's
daemon owns the filesystem, the PTYs and the search — proxying adds a hop and a second truth);
`thread/shellCommand` (*"runs unsandboxed with full access"*); `thread/rollback` (deprecated, and
*"does not revert local file changes"* — Fleet's revert is git-based).

**Completion authority is `turn/completed`.** Two traps, both confirmed live:

- **`turn/completed.turn.items` is a *summary*.** Governed by `itemsView`, it reported **one** item
  for a turn that produced four. A client rebuilding a transcript from it silently loses every
  tool call. The transcript is accumulated from `item/started` / `item/completed` and their
  deltas; `turn/completed` contributes only `status`, `error` and the timestamps.
- **The renderable command is `commandActions[]`, never `command`.** The raw field is
  `/usr/bin/bash -lc "sed -n '1,120p' note.txt"`; the same item carries
  `[{type:"read", command:"sed -n '1,120p' note.txt", name:"note.txt", path:"…"}]`. Codex
  classifies its own shell commands, and that array **is** the kind column. Render from `command`
  and every Codex tool row shows a `bash -lc` wrapper where Claude shows a clean `Read`.
  `commandActions` is an array — a piped command decomposes — so the summary uses the first
  meaningful action and the expanded body lists all of them.

An `error` notification is **not terminal**: the live ordering on a real failure was
`thread/status/changed{idle}` → `error` → `turn/completed{status:"failed"}`. `willRetry: true`
means Codex owns the retry and the turn stays running. `codexErrorInfo` is a **typed** taxonomy
and four members earn distinct UI: `contextWindowExceeded` (offer compaction),
`usageLimitExceeded` (the countdown row), `serverOverloaded` (*"model at capacity — try another
model"* beside a model-switch affordance, which is exactly what the live capture produced), and
`activeTurnNotSteerable` (fall back to a queued turn, silently).

**Subagents multiplex onto the same connection.** A Codex subagent is a full app-server thread on
the same stdio, so the notification stream is N threads interleaved and the adapter must
demultiplex. Fleet copies t3code's routing table verbatim as a **pure function tested against a
real captured wire trace** — the single best idea in that codebase — with unknown methods
degrading to `Parent`, never to silent loss. Three guards ride with it, each traceable to a
captured hazard: never register the root as its own child (it made t3code intercept the parent's
own `turn/completed` and hang the thread forever); child traffic can precede its registration, so
suppress thread-level notifications for unregistered threads **while still recording their live
turn id** so Stop reaches them; and `serverRequest/resolved` **always** reaches the parent,
whatever the routing says, because swallowing it makes approval cards stick forever.

Five server→client requests are implemented (`item/commandExecution/requestApproval`,
`item/fileChange/requestApproval`, `item/tool/requestUserInput`,
`item/permissions/requestApproval`, `mcpServer/elicitation/request`); the rest answer `-32601`,
loudly on the legacy `applyPatchApproval`/`execCommandApproval` pair, because seeing it means
Fleet took the legacy path — a version-gate failure by definition.

**Gate keys are the JSON-RPC request id.** t3code keys on `approvalId ?? itemId`
(`CodexSessionRuntime.ts:1966`) and that collides: `approvalId` may be null and several approvals
can share one `itemId` for zsh-exec-bridge subcommands.

**`availableDecisions` is authoritative and ordered.** Fleet renders exactly the options Codex
offers, in Codex's order, and never invents one. Where absent (older CLI) it falls back to
`[accept, acceptForSession, decline, cancel]`. Fleet also implements the amendment variants
t3code skipped — `proposedExecpolicyAmendment` echoed back as `acceptWithExecpolicyAmendment` is a
real affordance the user is otherwise denied. Fleet never *invents* an amendment; it echoes the
server's own proposal or offers nothing.

The file-change approval **carries no diff and no paths**. The changes live on the `fileChange`
item named by `itemId`. Fleet joins on `itemId`, shows a loading state if the item has not
arrived, and never renders `reason` as if it were the change.

**Codex's stderr is a signal, deduplicated.** `app-server` writes structured log lines to stderr;
only `ERROR` lines are surfaced, and each distinct failure is surfaced **once per process** — the
fingerprint drops the `url:`, `cf-ray:` and `request id:` segments that change between repeats, so
the background model refresh Codex retries every three minutes on a dead token is one transcript
row, not one every three minutes (the shipped store had 906 of them). A line naming a `401
Unauthorized`, a `token_revoked`, or an invalidated authentication token is **the signed-out
signal**: `account/read` still answers the cached account after a revocation, so this line is the
only thing that tells Fleet every turn is about to be refused. It publishes
`AccountChanged { SignedOut }` and the one `/login` notice, and never the raw sentence.

### 4.3 Divergence — where normalisation would lose something

The full 48-row table is in the harness spec; these are the rows where flattening the two
harnesses into one would destroy something the user must still see. Fleet carries the difference
into the UI rather than pretending it away.

| Axis | Claude Code | Codex | Fleet |
| --- | --- | --- | --- |
| "Always allow" scope | real, via `updatedPermissions` with a persistent destination | **not expressible** for exec/file approvals; survives only in MCP elicitation `_meta.persist` | the button is offered on Claude and **absent** on Codex. Advertising a button that silently means something weaker is the worse outcome. |
| Auto-approval by the harness | `--permission-mode auto`, labelled `auto-approves safe actions` | Codex can expose `auto_review` internally, but the normalized picker does not advertise an Auto mode | capability-declared mode lists prevent one harness's semantics leaking into the other. |
| Extra approval kinds | — | network-policy amendment, execpolicy amendment, permission-profile widening | implemented on Codex; the affordances simply do not appear on Claude. |
| Question secrecy | — | `isSecret: bool` | masked input, never persisted, never logged. Dropping it writes a credential into SQLite. |
| Non-blocking questions | — | `isBlocking: false`, plus async `agentMessage.questions` that never end the turn | must not mark the thread "needs you", must stay answerable after settlement. |
| Question auto-resolution | — | `autoResolutionMs` | a countdown, then the card goes read-only. |
| Live reasoning size | `system/thinking_tokens` ticks a live estimate | none live; only `reasoningOutputTokens` after the fact | `thinking · 132 tokens` ticking on Claude, `thought 6s` on Codex. Faking a live counter for Codex would be an invented number. |
| Reasoning channels | one | two: `summary[]` per index with part boundaries, and raw `content[]` | `summaryPartAdded` is a paragraph break; concatenating across it produces a run-on wall of text. |
| Cost of a control change | model/effort/mode are launch flags ⇒ **restart with resume** | per-turn or `thread/settings/update` ⇒ **never restarts** | the composer says "applies next turn" on Codex and shows a brief restart on Claude. Pretending a Claude effort switch is free makes it silently not apply. |
| Context-window denominator | published **only** on `result` | pushed live per model step | the Codex meter moves during a turn; the Claude one cannot. Fleet lets it move and does not fake the other. |
| Parked turn | a rejected window parks the turn and **no `result` ever arrives** | the turn fails with `usageLimitExceeded` and `turn/completed` still lands | `Waiting{UsageLimit}` with a countdown on Claude; a settled failed turn on Codex. The states genuinely differ. |
| Cancelling queued input on Stop | `cancel_queued: true` | queued turns are separate turn ids and **survive** Stop | Fleet says so in the Stop affordance rather than pretending both discard it. |
| Fork | from the tip only | `thread/fork {lastTurnId?}`, at any turn | fork-from-tip on both, fork-at-turn on Codex only. The menu item is present-and-enabled or absent, never present-and-lying. |
| Subagent accounting | `subagent_stats` says **why** one was refused (depth / concurrency / budget) | per-child token usage only | one footer; Claude's can explain a refusal and Codex's cannot. |
| Sandboxing | none — `--add-dir` grants scope | first-class: read-only / workspace-write / danger-full-access | "Ask before everything" means a **read-only sandbox** on Codex and merely "prompt me" on Claude. The mode's help text says which. |
| Thread title | none on any frame | `thread/name/updated` | Fleet's fallback runs for Claude and yields to Codex's when it arrives. |
| Skills | one skill per message, on the **last** content block, first character `/` | a **structured** `skill{name,path}` input part | Claude's placement rule is a real constraint; Fleet rewrites `$name` accordingly on Claude and emits a typed part on Codex. |

### 4.4 Authoritative signals

**"Authoritative" means: this and nothing else moves the state.** Everything in the last column is
allowed to make the UI more truthful and is forbidden from changing the state machine.

| Signal | Claude Code | Codex | Hints that must NOT be treated as authoritative |
| --- | --- | --- | --- |
| Session ready | the child surviving its spawn window (`system/init` only lands with the first prompt, and re-affirms `Ready` when it does; §4.1) | `initialize` result **and** `thread/start`/`resume` result | a spawn that has not yet outlived the window |
| Resume cursor | `session_id` on any **durable** frame | the Codex thread id | any `session_id` on `hook_started`/`hook_progress`/`hook_response` — those are **transient** and adopting one corrupts the cursor |
| Turn started | Fleet's own `submit` wrote a `user` frame with no turn open; or synthetic | `turn/started`, or the `turn/start` response | `thread/status/changed{active}`, which fires **before** `turn/started`; `session_state_changed{running}` |
| Streaming text | `content_block_delta{text_delta}` | `item/agentMessage/delta` | `assistant` snapshot frames (they backfill, never stream) |
| Streaming reasoning | `thinking_delta` | `item/reasoning/{summaryTextDelta,textDelta}` | `signature_delta` — **never display text** |
| Command output | `tool_result` text on a `user` frame | `item/commandExecution/outputDelta` (plain, untagged) | `command/exec/outputDelta` — a different family, base64, client-initiated |
| Tool started | `content_block_start{tool_use}` | `item/started` | `tool_progress` |
| Tool result | the `user` frame's `tool_result` + structured `tool_use_result` | `item/completed` — **the item is the result** | `turn/completed.turn.items` — a *summary* |
| Permission | `control_request{can_use_tool}` | `item/{commandExecution,fileChange,permissions}/requestApproval` | `activeFlags:[waitingOnApproval]` — cross-check, never a source |
| Question | `can_use_tool` for `AskUserQuestion` | `item/tool/requestUserInput`, or an async `agentMessage` with `questions` | `activeFlags:[waitingOnUserInput]` |
| Plan | `can_use_tool` for `ExitPlanMode`, or an `ExitPlanMode` block on an `assistant` snapshot | `item/completed{type:"plan"}` | `turn/plan/updated` — that is the todo list, not the proposal |
| Gate withdrawn | `control_cancel_request` | `serverRequest/resolved` | — (**answer nothing**) |
| Turn completed | **exactly one** `result` | `turn/completed{turn.status}` | `session_state_changed`, `message_stop`, the last assistant message, `thread/status/changed{idle}` (fires **before** `turn/completed`) |
| Turn aborted | `result` with `terminal_reason` ∈ {`aborted_tools`,`aborted_streaming`} | `turn/completed{status:"interrupted"}` | the interrupt RPC returning — a receipt, not a settlement |
| Retrying | `system/api_retry` | `error{willRetry:true}` | — (neither is retried by Fleet) |
| Process death | stdout EOF / non-zero exit | child exit / transport termination | stdout EOF **is never a turn boundary** |
| Attention | derived from open gates | `ThreadStatus.activeFlags`, cross-checked | — |
| Account | — (Claude publishes none, so the projection's `account` stays `None`) | the `account/read` **response** | `account/updated` and `account/login/completed`, which carry no email and are re-read triggers, never the account itself |

### 4.5 Capability gating and tolerant decoding

**Claude gates on `system/init.capabilities`, never on a version-string compare.** Observed:
`["interrupt_receipt_v1", "interrupt_cancel_queued_v1", "msg_lifecycle_v1"]`. Absent
`interrupt_receipt_v1`, interrupts are unacknowledged and Fleet relies on the eventual `result` —
degrade, do not refuse. Absent `interrupt_cancel_queued_v1`, Stop cannot cancel queued steers and
**the Stop affordance says so**. Absent `msg_lifecycle_v1`, **refuse**: `Unavailable`, terminal
fallback. This is the version gate the previous revision asked for and had no mechanism for.

**Codex gates on the parsed `userAgent` semver plus behaviour probes**, because it declares
nothing. A legacy `applyPatchApproval` ever arriving is a refusal. `availableDecisions` absent
degrades to the four-decision default. `optOutNotificationMethods` rejected degrades to
daemon-side filtering with one warning about event volume over a remote link.

**A harness version gate is never an IPC `PROTOCOL_VERSION` bump.** Remote links require an exact
IPC match, so a new harness capability is an additive capability string on the existing handshake.
An incompatible Fleet request or enum shape is different: it must bump the IPC version, as the
version-8 default-controls change does in §10.

Both harnesses add fields and enum values without a version bump — Codex demonstrably:
`rateLimitExceeded` and `misalignmentPolicyViolation` were added to `CodexErrorInfo` after the
pinned schema. t3code's client **silently swallows any notification whose params fail to decode**
(`client.ts:151-158`), so one new enum value can make a whole class of events invisible with no
error anywhere. **That is the highest-value thing to do differently.** Fleet's rules:

1. Every closed enum gets a catch-all — `#[serde(other)] Unknown` for unit enums, an
   `Unknown { #[serde(flatten)] raw }` arm for every tagged union.
2. **A decode failure produces a degraded event, never silence**: `AgentEvent::Unknown { method }`
   plus a counted, rate-limited warning carrying the method name and a structural fingerprint
   only. The UI can then say "1 event Fleet doesn't understand" instead of losing a tool call.
3. **Decode history per item, not per response** — `thread/read` must survive one unrecognised
   item in turn 3 of a 200-turn thread.
4. Unknown methods degrade to "someone sees it", never to silent loss. The known-noise list is
   **data**, not match arms, so it extends without a protocol change.
5. `Option<Option<T>>` only where "explicitly cleared" differs from "unchanged" —
   `account/rateLimits/updated` states that nulls do **not** clear a previously observed value.
6. **Absent is not the same as negative.** `ThreadProjection.account` is `Option<AccountStatus>`:
   `None` is *never reported* — Claude, or a log written before the field existed — and
   `Some(SignedOut)` is the harness saying it looked and found nothing. A surface that collapsed
   the two would draw `signed out` on every Claude thread. The same distinction is why
   `account/read` answering `{account: null, requiresOpenaiAuth: false}` reports **nothing**: a
   thread pointed at another model provider needs no OpenAI account and is not signed out.
   `AccountChanged` is serialized into the event log like every other event, and a build that does
   not know the tag drops it by rule 1 rather than failing the log.

### 4.6 Fallback

`spawn` fails with `HarnessError::Unavailable { reason }` and the reason is user-facing copy, not a
stack trace: binary not on PATH, installed but failed to run, not the harness at all, handshake
timeout, signed out (with JSON-quoted paths so they survive any shell), or too old for Fleet. Each
one names the **configured `agentBinaries` command**, because that is the string the user has to
change; the app shows the daemon's message verbatim rather than prefixing a default executable
name it may never have run. Every one names the same escape hatch: **`^s F` opens the agent in a
terminal on the same worktree**. The native tab does not half-work; it says why and points at the
fallback.

## 5. Rendering model

The app holds one `Entity<AgentThreadView>` per open agent tab, owning one memoised
`Rc<[Row]>` per thread.

**The transcript is a flat list of rows, never a tree of turn widgets.** A turn is an emergent run
of rows, not a container. Nested containers make variable-height virtualization and scroll
anchoring unsolvable; t3code's flat `MessagesTimelineRow[]` (`MessagesTimeline.logic.ts:309-392`)
is why its list is correct.

```rust
pub enum Row {
    User, Assistant, AssistantMeta, Reasoning,
    Work, WorkLive, WorkGroup, Subagent, Diff,
    TurnFold, TurnFooter, Plan, Gate,
    Checkpoint, Notice, Error, Working, Empty,
}
```

Every row carries `id: RowId` and `anchor: (Seq, u16)`. `RowId` is **stable across the life of the
thing it draws** and is what the virtualizer's recycling pool keys on. Two ids are shared on
purpose: `RowId::LiveActivity` is used by `WorkLive`, by `Reasoning` while streaming, and by
`Working`, so *thinking → tool A running → tool A done → tool B running* is **one row changing its
label**, not four mounts (t3code's `LIVE_ACTIVITY_ROW_ID`); and `RowId::Item(ItemId)` is shared by
a `Work` row and the `Diff` row under it so an update merging forward does not remount.

Update policy: `ThreadProjection::apply_described` returns `Applied::Text { item, stream,
appended }` only for an append to an existing open item and `Applied::Structural` for every other
accepted event. The client mirror carries that description into `AppState`. A bridge batch made
only of text descriptions does not notify `AppState`, run attention edges, or synchronize the
shell; it passes the borrowed authoritative projection and the descriptions directly to the
active thread view. The view copies only each appended suffix into its presentation projection,
feeds assistant prose through `MarkdownDocument::append`, replaces only the indexed row, and calls
`TranscriptList::patch_row`, which uses `ListState::remeasure_items(index..index + 1)` and never
`splice`. There is no projection clone, settled-turn count, deep comparison, grouping pass, or
linear item lookup on this path. Any structural member keeps the ordinary `AppState` notification,
clones the projection once, and prepares the complete row model. Text descriptions already
accepted earlier in that mixed batch are queued and flushed through the same single-row remeasure
before the structural model is installed; a final delta plus completion therefore cannot splice a
growing row. A delta that clears `retrying` also takes the structural path so its trailing notice
disappears in the same update.

The thread view holds a reveal buffer between the authoritative projection and its rows. With
motion enabled it wakes every `motion.reveal_tick_ms` (**16 ms**) and reveals UTF-8-safe chunks at
the rate needed to drain the burst within `motion.reveal_horizon_ms` (**200 ms**), feeding every
chunk through the same incremental Markdown and single-row patch path. `ItemCompleted`,
`TurnSettled`, a thread switch, and reduced motion flush the buffer immediately; `FLEET_HARNESS=1`
sets reduced motion, so harness snapshots remain deterministic. The stored foreground task stops
when the queue empties. The `AppState` projection remains truth throughout; only painted rows lag.

That distinction preserves `logical_scroll_top.offset_in_item` when the row under a frozen reader
grows; `splice` is reserved for structural events that rebuild grouping. Heavy row payloads are
shared (`Rc<MarkdownDocument>`, shared attachment/roster/footer slices, and `SharedString` tool
output/diff text, whose heap representation is `Arc<str>`), so cloning a `TranscriptRow` is O(1).
Assistant, reasoning, and tool items—including output hidden by `WorkLive`—retain a `row_of_item`
entry. Turn metadata is **withheld until the turn completes**, so the footer never moves under the
reader.

Assistant prose keeps the same row while streaming. Its `MarkdownDocument::append` path retains
every settled block and reparses only the final open block; the optional caret is a final text run,
not a sibling element. Paragraphs and table cells shape as one wrapping text element, while fenced
code preserves its lines in a horizontal, axis-restricted scroller so vertical wheel input still
belongs to the transcript.

Tool rows keep their 30 px geometry while streaming. Five states and no more —
`Running | Done | Failed | Denied | Stopped` — plus `Severe`, reserved for a runtime error or a
broken side effect, *"not that a command exited nonzero"*. A `git grep` finding nothing is not
red. **Exit codes are structured fields, rendered explicitly**: `exit 1`. Fleet never
substring-matches English error text to infer failure, which is what t3code does
(`session-logic.ts:1241`) and which is locale- and shell-dependent.

A live tool row is present-tense by rule, not by lookup: a *completed* command inside the live row
still reads `running cargo`, never `ran cargo`, so the row never flickers between tenses while the
turn is alive. Adjacent settled rows collapse to one generated summary —
`read 3 files and ran 2 commands` — and **group failure is neutral by default**: a group holding
one failure and three successes draws no danger mark unless the *latest* entry failed. The failure
always reaches the accessibility label regardless.

A **failed** row never folds, and a subagent row never folds while its work is live — *"workflows
outlive their launching turn … folding the CTA when the turn settles makes a still-running fleet
invisible"* (`MessagesTimeline.logic.ts:575`).

**The duration in the fold is the model's, not the wall clock's.** Every interval a gate stood
open is the user's time and is subtracted from the harness's figure by the reducer. That is why
the fold says `worked 22s` on a turn that was on screen for four minutes.

An interrupted turn caused by the user reads `you stopped after 12s` with footer word `stopped`;
a stopped session reads `stopped after 12s` / `stopped`; a provider-exit abort reads
`cut off after 12s` / `cut off`. Older records with no abort reason keep the user-stop copy.

Diffs render as their own row under an expanded edit row, so an expanded diff never inflates the
tool row's own measurement, drawn by `fleet_lazygit::diff_view::DiffView` from unified-diff text
(ADR 0010 — the kit gains no `fleet-git` dependency). Claude supplies `old_string`/`new_string`
and the daemon synthesises the diff; Codex supplies `fileChange.changes` on the item.

**Reasoning is rendered, collapsed, behind one switch.** t3code drops it at ingestion
(`ProviderRuntimeIngestion.ts:1479`) and shows one shimmering word; Fleet keeps the shimmer *and*
streams a `thought 12s` block, because Fleet's users watch two harnesses and Codex's reasoning
summary is designed to be shown. It auto-collapses when the turn's terminal assistant message
arrives.

**Render prepares nothing.** Every row is a projection memoised behind a revision key; `render`
composes prepared values and nothing else (`APP-CONTRACTS.md`, "Render prepares nothing"). Code
fences are **not** syntax-highlighted while streaming, and a partial fence is neither read from
nor written to the highlight cache — it must never poison it.

Seven things must never move under the reader: turn metadata; the live row's height (pinned at
`row_h` from turn start, including the empty startup window); tool row geometry; a code block's
highlighting; a standalone image's slot (reserved only for an image that is the *sole* content of
its block, because *a placeholder taller than the image would move the page more than the image
does*); everything above the viewport; and the composer's own height.

`[u] revert this edit` and `[u] revert turn` restore files from a **Fleet-owned checkpoint**: a
hidden git ref of the worktree recorded before each turn, and of a file before the first edit that
touches it. Both halves of the daemon side are built (`services/checkpoints/` plus the two capture
call sites in `AgentSessionManager`); the affordance itself is the app's and is drawn only where a
checkpoint exists, because a drawn affordance that does nothing is worse than an absent one
(`DESIGN-SYSTEM.md` §7) — `AgentCheckpoints` is what it asks. `[u] revert turn` is drawn today;
`[u] revert this edit` is not, because `TurnCheckpoint` names the *turn* a file-scope checkpoint
was taken in and not the item it covered, so a tool row has nothing to key on (§13 phase 8).

Four decisions hold it together, and each one is a refusal of an easier design:

- **The checkpoint is Fleet's, not the harness's.** It is taken before Fleet asks a harness for
  anything, so it exists whether the turn completed, crashed mid-edit, or was interrupted, and it
  is decoupled from harness completion entirely. Codex's own `thread/rollback` is **not** used:
  it is deprecated upstream and *"only modifies the thread's history and does not revert local
  file changes"* (§4.3) — the opposite of the trade `[u]` is asking for.
- **A revert restores files and nothing else.** It never touches the conversation, `HEAD`, a
  branch, the user's index, or the stash. Reverting a working tree and rewinding a model's context
  are different operations, and conflating them is how a user loses work they meant to keep. The
  model still remembers the edit; the files no longer have it, and the user says so in the
  composer if it matters.
- **The ref namespace is the store.** `refs/fleet/checkpoints/<thread>/<ordinal>-<scope>-<turn>`
  is the only record: outside `refs/heads`, `refs/tags` and `refs/remotes`, so it never appears in
  `git branch`, never appears in Fleet's own ref listings, and is never pushed. The `checkpoints`
  **table** is the projector's read model of `AgentEvent::Compacted` and is erased and rebuilt by
  `rebuild_thread`, which would take a checkpoint index with it and leave live refs
  unattributable — so there is no second truth to reconcile. Every capture and restore runs
  against a scratch `GIT_INDEX_FILE`.
- **Garbage collection is per thread, never per prefix.** A deleted thread's refs are deleted with
  it, and an hourly sweep catches what an interrupted deletion left behind. The sweep's live set
  comes from the fallible thread listing, never from the one that answers an empty vector when the
  database cannot be read: an empty live set means "every checkpoint is an orphan".

## 6. Decision surfaces

### 6.1 Placement

**Permission approvals and model questions render in a drawer docked to the top edge of the
composer. The proposed plan renders as an in-transcript card with no buttons. Nothing is ever a
modal.** This reverses the previous revision of §2 and §5, deliberately:

- **A transcript card can be scrolled off screen while it owns the keyboard.** Fleet's decision
  contexts derive from daemon state, so `y` answers a permission whether or not the card is
  visible. Combine that with `^s [` scroll mode — which exists precisely so the user can read back
  through what the agent did — and the old design has a keyboard-owning surface the user cannot
  see. That is a modal with the chrome removed. The dock is always on screen by construction.
- **Deciding requires reading the context.** The command under review usually only makes sense
  with the twelve rows above it. A docked drawer lets the transcript scroll freely behind the
  decision; a card pinned as the last row forces a choice between reading and deciding.
- **The composer is the right thing to repurpose.** While an approval is open the composer has
  nothing to do; while a question is open the composer **is** the free-text answer field.
- **The plan is different in kind** — a durable artifact the user will scroll back to, quote and
  copy. It belongs in the transcript. Its *verbs* belong on the composer, because whether you
  implement or refine is decided by whether you typed anything.
- **History is not lost.** Every resolved request leaves a one-line `Gate` row at the position it
  was asked.

One slot, one occupant, strict priority **approval > question > plan-ready**. When several
requests are pending only the head is actionable and the rest are a `1/N` counter, sorted
ascending by creation time, and there is **no "approve all"**.

### 6.2 Permission approvals

The payload is a code well, `whitespace: pre`, bounded to about three lines with its own scroll in
both axes, focusable so a keyboard user can scroll it, and **never truncated and never
line-clamped**. It is the **invocation**, never the model's prose about the invocation, because it
is what `[e]` seeds the composer with. For an edit approval Fleet shows what t3code cannot: **the
diff**, joined by `itemId` and rendered by `DiffView` at bounded height.

| Action | Copy | Claude | Codex |
| --- | --- | --- | --- |
| `[y]` | `allow once` | `{behavior:"allow", updatedInput}` | `accept` |
| `[a]` | `allow for this session` | allow + suggestions rescoped to `destination:"session"`, or a whole-tool session rule when none is offered | `acceptForSession` |
| `[n]` | `deny` | `{behavior:"deny", interrupt:false}` | `decline` — **the turn continues** |
| `[esc]` | `deny and stop` | `{behavior:"deny", interrupt:true}` | `cancel` — **the turn is interrupted** |
| `[e]` | `edit` | offered — the allow carries the corrected `updatedInput` | **not offered**; the key is unbound and the hint absent |
| — | always / forever | not offered for commands or file changes on either harness | Codex's wire has no "always"; t3code downgrades it silently |

The decline/cancel distinction is not cosmetic and both must be reachable. **The word "always"
never appears** on a command or file-change approval. There is no directory or project scope in
v1.

**`Enter` is not bound.** A queued Return keystroke must never approve a shell command — the one
t3code property worth keeping exactly. Keys are bare letters in a derived context
(`Agent > AgentDecision > AgentPermission`) so they cannot fire anywhere else, there is no default
focus and no focus ring, and the status bar mirrors the card's hints **from the same source** so
it can never advertise a scope the card does not offer.

Cancellation, death and staleness: a withdrawn request closes as `Withdrawn` with **no** response
sent; a turn ending with a native question open resolves it as dismissed, because a terminal turn
cannot accept native callback answers, while async questions survive; session teardown emits a
synthetic resolution **on both harnesses** (t3code does it for Claude and not for Codex, leaving
Codex approvals pending forever — a real bug surface); a reply after a restart fails with a
**typed** `GateStale` error, not a substring match on English; and a transient failure **restores
the optimistic clear to pending** unless a terminal event already closed it. There are **no
timeouts** — a request is open until answered, withdrawn, or declared stale. An open gate **vetoes**
thread settlement.

### 6.3 Model questions

Normalised `Question`: a stable id, `header`, `prompt`, `options`, `multi_select`, `allows_other`,
`is_secret`, `blocking`. Claude keys answers by the **exact question text** (the CLI looks them up
by it) and Fleet keeps that verbatim beside its own id; Codex keys by **question id**. Claude has
an explicit `multiSelect`; Codex makes it implicit through `answers: string[]`. `is_secret` and
`blocking` are Codex-only and dropping either is a bug: the first writes a credential into
SQLite, the second falsely marks the thread as needing you.

Codex has a second, distinct channel: an async `agentMessage` with `questions[]` where **no
JSON-RPC request is pending and the turn does not end**. Fleet synthesises a gate with a
**deterministic** id `codex-async:<thread>:<item>` so a replay or reconnect re-derives the same
gate rather than duplicating it, and the answer is delivered as a **new message**.

### 6.4 Proposed plans

The only rich decision artifact in the transcript, and it carries **no buttons**. The title is the
plan's first Markdown heading, promoted out of the body and removed from it. Actions live on the
composer: `[y] implement`, `[n] refine`. One plan per turn, upserted, keyed `(thread, turn)`.
Claude's arrives as an always-denied `ExitPlanMode`; Codex's as an `item/completed` of type
`plan`.

### 6.5 Testing a gate without a vendor CLI

`fleet-harness agent --provider <claude|codex> --transcript <file>` speaks either wire protocol
from a scripted document (`docs/TESTING-HARNESS.md` §5), so a gate can be exercised with no vendor
binary, no network and no token. It emits exactly the two frames this section describes —
`control_request { subtype: "can_use_tool" }` for Claude, and
`item/commandExecution/requestApproval` or `item/fileChange/requestApproval` for Codex — and waits
for the answer Fleet sends back, so a declined gate and an accepted one are both reachable from a
scenario. The `agents` fixture installs it on `PATH` under the vendor's own name.

A scenario therefore exercises a gate entirely through this document's own surfaces: open the
worktree, `key ctrl-s A` for a native thread, `assert focused == agents.composer` and `type` a
prompt, then
`await agents.threads[0].pending_gate == "permission"`, `click agents.approval.allow_once`, and
`await agents.threads[0].pending_gate absent`. The approval controls carry the frozen target names
`agents.approval.{allow_once,allow_always,deny,deny_and_stop,edit}` (§6.2); a model question's
options are deliberately unnamed, because §6.3's options are answered by number and a name would
imply a stable identity they do not have.

After projecting an event, `project_event` also runs the delegation transition when that thread
is a delegation child or caller; a gate opening, resolving or withdrawing therefore moves both
the ordinary thread projection and the delegation record in the same writer transaction (§15).

## 7. Controls

### 7.1 The model of a control

```rust
pub struct ModelSelection {
    pub instance: InstanceId,   // routing key, an OPEN slug — parsing an unknown one must succeed
    pub model: SharedString,
    pub options: SmallVec<[OptionSelection; 4]>,   // canonical {id, value: String|bool}
}
```

Options are **declared by the harness** in exactly two descriptor kinds, `select` and `boolean`.
There is no slider, no number, no free text. Reasoning effort, thinking, context window and
service tier are all one of those two, so adding an effort level is a data change rather than a
code change. **Fleet never hardcodes a Codex effort ladder** — the legal set is per model from
`model/list.supportedReasoningEfforts`, with the harness's own `description` strings.
`SessionConfigured.models` projects `{ id, display_name, efforts: [{ id, description }],
default_effort }` through the store and `AgentSessionView`; `^s e` reads only the selected model's
descriptor and a newly selected model starts at its advertised default. Claude's current frame
does not report a default effort, so `None` means the harness default. A later Codex
`skills/changed` is a
`MetadataChanged.skills` observation, so `$` refreshes without pretending the session restarted.

| Control | Claude | Codex | Mid-thread? | Mid-turn? | Key |
| --- | --- | --- | --- | --- | --- |
| Model | launch flag; any selection change restarts with the resume cursor | sent on each `turn/start`; no restart | yes | no | `^s m` |
| Reasoning effort | `--effort`; restarts with resume cursor | `effort` on `turn/start`; no restart. A non-empty **string**, not an enum | yes | no | `^s e` |
| Context window | a `select` that **rewrites the model id** via a suffix (`claude-opus-5[1m]`); also the meter's denominator | not offered | yes | no | `^s e` |
| Fast mode / service tier | a setting; restarts | `serviceTier`, a `select`; no restart | yes | no | `^s e` |
| `ultrathink` | **not an effort** — a prompt prefix, skipped when the prompt starts with a slash command | n/a | yes | no | `^s e` |
| Access mode | one `--permission-mode`; restart with resume | approval/sandbox controls updated in place. **The reviewer is re-sent every time**, or a prior value stays sticky | yes | no | `^s t` |
| Build ⇄ Plan | `--permission-mode plan`, restoring the **base** mode on leaving; restart with resume | the normalized Plan mode updates approval/sandbox controls in place | yes | no | `⇧⇥` |
| Compaction | a slash command: send `/compact` as a turn, and synthesise the boundary if the turn settles without one | native `thread/compact/start` | yes | no | — |
| Instance (same driver) | restarts with resume cursor | same | yes | no | `^s m` |
| Instance (different driver) | **rejected** — a Claude thread cannot become a Codex thread | same | never | never | — |
| Account | not offered — Claude has no account method Fleet can drive | `/login` opens Codex's ChatGPT browser flow, `/logout` signs out; no restart, and the account is **process-wide** for that `CODEX_HOME` rather than per thread | yes | no | `/login`, `/logout` |

`fleet subagent run --effort <EFFORT>` sets the same reasoning-effort control on a delegated child
at launch. It takes a free string rather than an enum for the reason above, and it is accepted
**without** `--model`: the child then keeps the provider's default model and gets the requested
effort (§15).

The access picker is capability-driven. Claude declares `Ask`, `AcceptEdits`, `Plan`, `Auto`,
`DontAsk`, `FullAccess`; Codex declares `Ask`, `AcceptEdits`, `Plan`, `FullAccess`. The labels are
`asks before edits`, `accepts edits`, `plans before editing`, `auto-approves safe actions`,
`denies unlisted tools`, and `full access`. The daemon rejects a mode absent from the live
capability list instead of letting an adapter approximate it.

**Three orthogonal Codex axes Fleet must not collapse internally:** approval policy, sandbox
policy, permission profile. The normalized picker maps Ask/Plan to untrusted + read-only,
AcceptEdits to on-request + workspace-write, and FullAccess to never + danger-full-access.

**Nothing applies mid-turn.** Every control writes to `ControlDraft`, which is view-local and
per thread, and takes effect at the next send; it is not sticky global memory and does not survive
recreating the view. The applied thread projection remains daemon-persisted. On send, controls and
the message enter one FIFO bridge mutation lane: each control must receive its daemon
acknowledgement before the message is admitted, so a Claude restart completes at the turn boundary
instead of racing the new turn. Before Claude restart work begins, `session = Starting` is
published optimistically so the tab shows `starting…` in the same frame rather than after the round
trip. The manager first resolves any open gates as provider-closed; restart teardown lifecycle
events then belong to the retired harness generation and are not projected into the thread
transcript. If replacement startup or old-process shutdown fails, the manager records `Error` and
empties the unusable provider slot; the next eligible open or send lazily constructs a new provider
from the last controls that were actually persisted.

### 7.1.1 Defaults for new threads

`config.nativeAgents.claude` and `.codex` each contain `{ mode, model, effort }`. In a successfully
loaded configuration, an absent `nativeAgents` section resolves to `{ mode: full_access, model:
null, effort: null }`, so existing installations keep the documented no-prompts/no-sandbox
("yolo") default. If the configuration file cannot be read or parsed, new threads instead fail
closed to `{ mode: ask, model: null, effort: null }` while executable names retain their packaged
fallbacks, and the daemon both warns and records that degraded choice as a thread notice.
`AgentThreadCreate.mode` is optional and the app omits it; the daemon resolves omitted mode/model
from the matching harness defaults and persists the resolved values. Per-thread picks remain
available through the controls above.

### 7.2 The composer

**A message sent while a turn runs is a steer, dispatched immediately. There is no queue and no
`QueuedMessage` row.** The shipped queue lives in a GPUI view: lost on app restart, invisible to
`fleet agent`, and it re-implements what both harnesses already do. Claude coalesces a second
`user` frame into the same turn and reports how many were folded via `queued_turn_count`; Codex
takes `turn/steer{expectedTurnId}` and answers `activeTurnNotSteerable` when it cannot, so the
queue-vs-steer question is **answerable by the harness rather than guessed**. A steered message
renders as an ordinary user bubble with a leading `↳`, and Stop kills the turn.

Block order in a Claude submit is load-bearing: `[leading text?] … [images] … [final text]`. The
final text block goes **last** because the CLI only reads a streamed user message as a
slash-command invocation when the last block is text; leading with the text made every
image-carrying turn fall back to a plain prompt and a hand-typed `/skill args` reach the model
unexpanded (`ClaudeAdapter.ts:1538-1549`).

The user bubble shows **what the user typed**. Attachment manifests, `@file` expansions and
Fleet's own prompt prefixes are stripped for display and for `↑` recall, and kept verbatim for
copy.

**A send the daemon refuses is a failed bubble, never a stuck one.** The optimistic bubble is
reconciled by its client-minted `ItemId` (§9.2), so what the user typed never has to match what
the daemon stored. When the `AgentSend` request comes back an error — the thread is not live, the
harness would not take the prompt, the transport deadline passed — the bubble turns failed, the
daemon's sentence is said once as a notice, and the composer is free: a failed bubble counts as
neither work nor an unacknowledged send, so it blocks nothing and spins nothing. There is no retry
key; the text is one `↑` away.

The composer wraps every logical line to its resolved value-column width, breaking an unbroken
token at a character boundary rather than widening the panel. Its caret, pointer hit-testing,
drag selection, double-click word selection, visual `↑`/`↓` and unmodified `Home`/`End` all read
the same revision-tagged wrapped layout; a key arriving after an unpainted edit falls back to the
logical line instead of consulting stale geometry. Modified `cmd`/`ctrl` line motions remain
logical-line motions.

Pasted `\t` is stored and submitted unchanged, while rendering expands it to the design-system
`TAB_WIDTH`; other non-printing controls are discarded and CRLF is normalized. Plain `Enter`
while an IME marked range is open is consumed without submitting, leaving the platform to commit
the preedit. The app's global Send/Steer actions must make the same `is_composing()` check because
an action binding wins before the input's own key listener.

The surface grows through eight visual rows. Beyond that it clips, draws an internal scroll thumb
and owns the wheel so the transcript behind it does not move. A per-logical-line shaping cache is
validated by line text, wrap width and font; editing one line keeps the other shaped lines and
their stable `SharedString`s.

## 8. Storage

**One SQLite database per daemon at `$FLEET_HOME/agents/state.sqlite`**, holding the event log and
every read model. This replaces the NDJSON-log-per-thread plus whole-file `index.json` design
wholesale. ADR `0013-sqlite-agent-transcripts.md`.

The old design fails on five axes that all show up as visible latency: `load()` is O(whole
thread), so opening a 4 000-turn thread parses every byte of every delta and there is no "last N";
a cursored open is *also* O(whole thread), paid over SSH after every reconnect; daemon start
replays every thread's whole log, so startup grows without bound; every event costs four syscalls
of synchronous `std::fs` on a tokio worker **under a `std::sync::Mutex`**, which is a direct
violation of `rust-async-background-work` Rule 10; and there is no queryable projection, so "which
threads have an open gate?" requires replaying every thread.

| Decision | Rejected |
| --- | --- |
| `rusqlite` with `bundled`, in `fleet-daemon` only, no new crate | `sqlx` — its `Transaction` is `async` and compiles fine held across an `.await`, which under a single writer is a deadlock the type system does not catch. Its compile-time SQL checking needs a build-time database, and `make test` builds `fleetd` first with no database available |
| One owned writer thread named `fleet-agent-db`, one RW connection, an `mpsc` inbox and a `oneshot` reply | `spawn_blocking` — a task that never returns occupies a slot for the daemon's life, in a pool shared with PTY and git work |
| A 2-connection read-only pool behind a semaphore, used from `spawn_blocking` | one shared handle; under WAL a reader sees a consistent snapshot while the writer commits, which is what makes an atomic `(window, watermark)` read possible with no lock |
| Projections **synchronous with the append**, in the same transaction, committed before broadcast | an eventually-consistent catch-up projector on the write path. By the time the append returns, every read model reflects it — that is what makes reads instant |
| The projector cursor is **per thread**, not global | t3code's nine global cursors; Fleet's `Seq` is already per-thread, which is better for a per-thread subscription |
| The pagination cursor is `(thread_id, before_seq)` — one content-derived integer | t3code's `(anchor_timestamp, turn_id)` keyset, which exists only because its sequence is global and its rows get rewritten |
| Tool output and diffs are **never spilled to a blob table**; the item row carries a bounded head+tail window, the log carries the full bytes, the client asks for the rest by request | `checkpoint_diff_blobs`, which t3code built in migration 003 and abandoned |
| Attachments on disk, referenced from SQL, swept on start and GC'd after commit | base64 in the row |
| `PRAGMA busy_timeout` set **first**, then `journal_mode = WAL`, `synchronous = NORMAL` | setting `journal_mode` first, so the conversion fails rather than waits |

Tables: `agent_events` (the log and the only truth, `UNIQUE(thread_id, seq)`), `threads` (the list
row, with **denormalized** `open_gate_count`, `running_turn_id`, `head_seq`, `projected_seq` so the
list read never touches `items`, `turns` or `gates`), `turns`, `items` (with append-only `text` and
`reasoning` columns concatenated **in SQL**, which collapses thousands of delta rows into one row
for reads while the log keeps every delta for replay), `gates`, `checkpoints`, `sessions`,
`item_attachments`, `seen` (the monotonic per-installation read cursors written by
`AgentMarkSeen`), `closed_threads` (the per-installation closed-tab markers),
`agent_events_quarantine`, and `fleet_migrations`. No foreign keys — deletes are explicit
multi-table statements in the projector, which is what you want when you also have to delete
files.

Migration 3, `delegations`, adds two tables and three nullable columns to that schema.
`delegations` is the durable caller-to-child record: identity and token hash; caller thread, turn
and item; unique child thread; provider, depth, brief, expectation and eager policy; lifecycle,
result and delivery state; nudge/recovery counters; headline; and creation/finish timestamps.
`delegation_outbox` is the durable work queue for `deliver`, `nudge`, `settle`, `recover`,
`cancel_children` and `mirror`, with a partial index over rows whose `done` timestamp is null.
`threads.parent_thread_id`, `threads.delegation_id` and `threads.stop_cause` preserve the child
relationship and distinguish a user Stop from a provider exit. The delegation record is not
rebuilt from the transcript log: rebuild and quarantine leave both delegation tables alone, while
the service derives and advances only the lifecycle status columns from thread events.

Migration 4 adds the delegation submission state. Migration 5 adds `closed_threads`, keyed by
`(client_id, thread_id)` with `closed_at`, so one installation's strip does not change another's.

Three columns exist to keep the denormalized `attention` byte-identical to what
`ThreadProjection::attention` derives, rather than approximately equal to it: `threads.retrying_json`
(a provider retry is what separates a thread that is working from a thread that is idle, and the
reducer clears it on the next event of any other kind), `gates.blocked_since` (a run of overlapping
gates is **one** wait, so answering the first must not restart the clock for the second and the
turn footer must not be charged twice), and `turns.gate_blocked_ms`, which the resolution charges.

A fourth, `threads.turn_json`, carries the reducer's `TurnState` verbatim so the list row holds the
turn **and its identity** without a second query. `running_turn_id` alone cannot: `Completed`,
`Interrupted` and `Failed` all clear it, and re-deriving which of the three a settled turn was from
`last_outcome` is ambiguous — an authoritative `Interrupted` result and an aborted turn write the
same outcome. `AgentThreadList` is one `SELECT` against `threads` and joins nothing, which is the
whole point of the table.

**Daemon start replays nothing.** The manager holds reducer state only for the threads something is
using: a thread is hydrated on first use — an open, a send, a resume — under a hydration gate, so
one thread is never built twice and two writers never mint the same sequence. Start-time work is a
census of two index lookups taken on the writer's connection before the writer thread exists, and it
is worked through **in the background, one thread at a time**: a thread whose `projected_seq` lags
its `head_seq` is replayed through the same projector a live append uses, and one that cannot be
replayed is marked `session_state = 'error'` while the daemon starts anyway. Taking the census
before anything can write is what makes the pass safe — a thread created afterwards can never be in
it, so a live thread is never mistaken for an orphan of the previous run. For each orphan it
settles, the repair pass also publishes the settled summary so a client that connected before the
pass reached that thread sees the tab change without opening it.

`index.json` is gone, and with it the whole-file rewrite on every metadata change: a record is one
upsert on one row, which touches no projected column.

`agent_events_quarantine` is where `truncate_after` puts the events it drops. A transaction has no
torn tail, so the NDJSON `events.ndjson.broken-*` machinery is gone — but the other reason it
existed is not: an event that decodes, is in sequence, and is then refused by the projection on
replay has to leave the log so the next append does not collide with it, and it is the only
evidence of why the replay stopped. It is moved with its reason, never deleted, and the thread's
read model is rebuilt from the log that remains in the same transaction.

Retention: no compaction, no `VACUUM`, no prefix delete. Deletion happens per **thread**, never
per sequence prefix, because a prefix delete invalidates the meaning of a projector cursor.

`state.sqlite` is **not** part of `PersistedState`. It gets its own migration ladder and its own
failure mode: a database that cannot be opened or migrated is fatal **for the agent service** — the
manager holds the open failure instead of a store and answers every agent request with it, loudly
and once. It is deliberately not fatal for the daemon: `Services::build` is infallible across twenty
call sites, and taking terminals, jobs and worktrees down over an agent transcript database is a
worse failure than refusing agent work. The service still never runs with half a truth, because it
refuses all of it. Making the whole composition fallible, so the process exits the way an unreadable
`state.json` makes it exit, is a follow-up that belongs with `Services::build`, not here.

The one-shot NDJSON import is **not** a migration slot, though ADR 0013 sketched it as one. It runs
at store construction on the writer's own connection, before the writer thread is spawned, because
it needs one transaction *per thread* — so one unreadable log costs one thread — and because it moves
files, which has to happen strictly after the commit that made the rows durable. Both are the
opposite of what a single migration transaction gives you. It imports the readable prefix of a torn
log, records the tear as a `Notice` in the transcript, quarantines the rest, and **moves** the file
to `agents/imported/` rather than deleting it; a log whose header names a schema this build does not
implement is refused whole and left exactly where it is. Idempotency does not depend on the move
having worked: a thread that already has rows in `agent_events` is skipped.

Four disciplines survive from `store.rs` and must be reproduced: a header-versioned log, torn-tail
quarantine rather than data loss, selective durability, and restart-recovery-as-appended-events. A
thread the log leaves `Starting` or `Running` is an orphan and is settled **explicitly, as
appended events**, never as a silent state edit.

## 9. The live path, the client mirror, and remote

### 9.1 Coalescing

Two windows on the broadcast path, and no others: **16 ms merge** for adjacent `ContentDelta`
events with the same item and stream, and **50 ms last-wins** for `ItemUpdated` per item. The
earliest live deadline wins when both kinds share a batch. Every other event — a tool completing,
a turn settling, a gate opening — **wakes the select, flushes the pending window first, then emits
unchanged with zero added latency, by rule**. Paused-clock tests assert the clock does not advance
and terminal events are neither dropped nor reordered.

The harness→provider and provider→manager compatibility channels remain unbounded to avoid the
operation-gate deadlock described at their type definitions, but the pair forms one **coalescing
drain**, not two sleeping queues: the forwarder continuously empties the first into the second,
and the manager select drains that until the earliest 16/50 ms deadline. Thus streamed text
occupies one pending batch rather than one retained event per token; structural events bypass the
window immediately.
Assistant text is merged at the transport boundary and then revealed by the active view over at
most the 200 ms presentation horizon. A terminal item or turn event flushes that remainder before
its structural row preparation, so terminal signals still have zero added latency.

Reducer `accepts`/`apply` failures are `Validation` errors naming the thread and normalized event
kind, never the payload. Only SQLite/store failures are `Fs`; dispatch therefore adds filesystem
path wording only to actual storage I/O failures.

Backpressure: a budget overflow emits `Event::AgentResync` and the client re-opens with
`after_seq`. Never a stall, never an OOM, **never a silent drop**.

### 9.2 The client mirror

The app keeps **no on-disk cache**; its warm tier is a 5-minute in-memory map and the daemon's
SQLite is the cache. On open it paints the retained `Arc<ThreadProjection>` synchronously, and a
`Live` status stays `Live` so no label flashes. Optimistic sends use a **client-generated `ItemId`
that *is* the server id**, so reconciliation is by id with no temp-id swap and no matching
heuristic. "Sending" clears on a **field diff** against a pre-send snapshot — any server-visible
movement clears it — not on a correlated ack, which a *steer* would never produce.

Every connection replacement — first connect or reconnect, whether fleetd restarted or not —
re-opens every projection the client still holds from that projection's own `last_seq`. A catch-up
open never launches a provider; if the daemon no longer has that cursor, the client falls back
once to a fresh bounded newest-window open. A replay answer — the ladder admitted `(cursor, head]`
— carries no transcript on purpose and is applied **onto** the projection the cursor came from;
only a windowed answer replaces a projection, and a replay for a thread the client no longer
holds is a gap that re-opens the newest window.

### 9.3 Remote

> **The remote daemon owns the transcript and the sequence. The local daemon is a re-framing proxy
> that also maintains a durable read-through mirror in its own `state.sqlite`. The app never knows
> whether a thread is local or remote.**

The existing routing model is correct and tested (`tests/agents_remote.rs`) and survives
unchanged. The **no-copy rule does not**: `AgentThreadOpen` used to ship the whole
`ThreadProjection` over SSH against a 16 MiB ceiling with no compression, and past that ceiling it
became permanently *undecodable* — a codec error, not a truncation.

Read cursors follow the reader across this boundary. The source daemon retains the originating
installation identity while routing `AgentMarkSeen`; the owning daemon persists that identity's
cursor, and the bounded window carried back through the mirror includes its `seen_seq`. The mirror
never turns a remote reader into one shared global cursor.

A mirrored thread is stored in the **same tables** as a local one with one nullable
`threads.owner_host` set, because a mirrored thread is a prefix of the owner's log with the
owner's own `seq` values. That buys the property that makes remote feel local: **`AgentThreadOpen`
runs the same SQL whether the thread is local or remote.** One snapshot query, one cursor codec,
one row model, one code path in the app.

`mirror_head_seq`, `mirror_oldest_seq` and `mirror_synced_at` are the only mirror-specific state
and they live on `threads`, so the list read stays one statement. The three columns mean:

| Column | Meaning on a mirrored row |
| --- | --- |
| `head_seq` / `projected_seq` | the prefix this daemon holds — **identical** in meaning to a local thread, which is what keeps the boot census, the rebuild and every read unbranched |
| `mirror_head_seq` | the **owner's** head as last reported, so the admission ladder compares without a round trip |
| `mirror_oldest_seq` | how far back the prefix reaches; `NULL` is "to the start", the only shape an append builds |
| `mirror_synced_at` | when the owner last confirmed content |

The mirror is a read-through cache, **never a replica**: never consulted for a write, never merged
field-by-field, and any disagreement resolves in favour of the owner. Four authority rules, each
with a test in `services/agents/manager/tests/mirror.rs`:

| Rule | Enforced by |
| --- | --- |
| A thread with `owner_host` set may only be appended to from events received on that host's link — two writers on one sequence space is silent corruption | `store::mirror::admits_append`, a pure function asked once on the pre-flight read and again inside the write transaction |
| No harness process is ever started for a mirrored thread | the owner check in `resume_runtime` and in `create`, plus the orphan-settlement skip in `hydrate` — settling an orphan *appends events*, which is the same violation wearing a different hat |
| A mutation routes upstream and is never applied locally on optimism | the router classifies by owner from its id map **and the snapshot mirror as the fallback** — so a verb on a mirrored thread is forwarded even across a local restart, before any list or open re-registers the id, exactly as `host_of_worktree` already resolves a worktree; the local manager's `ErrorKind::Remote` refusal is then the backstop for the narrow window before the owner's first snapshot reaches the mirror, so an optimistic local apply never happens |
| The mirror never fabricates a `Synchronized` | a mirrored answer carries `synchronized = false` unconditionally, and only the owner's own `Event::AgentSynchronized`, forwarded verbatim, moves a client to `Live` |

The local router drains everything immediately ready from each host link, bounded at **64 events
or 256 KiB** per pass. Adjacent events for one thread form one ordered `mirror_append` and one
SQLite transaction; another thread or a structural event ends the run, so interleaved threads
make progress and link order stays observable unchanged. Republishing remains strictly
commit-before-visible. If a run contains a sequence gap, the mirror commits and republishes its
gap-free prefix. With a window-capable owner it withholds the suffix and starts the ordinary
window refill; the refill's `AgentWindow` announcement makes clients re-read the now-complete
durable prefix. With an older owner it republishes the suffix unchanged and leaves repair to the
plain proxy path. Each drain emits one debug record with event, byte, group, publishable-event and
resync counts, never payloads. The wire remains one event per frame; batching exists only between
the link receiver and local mirror.

Warm mirror, remote thread: the local daemon answers **entirely from the mirror in ~1 ms** with
`synchronized = false`, the app paints the full transcript as `Cached`, and in parallel the local
daemon asks the owner for `(after_seq, head]` — where `after_seq` is the prefix's own head, so a
caught-up mirror transfers nothing. **Zero transcript bytes cross the link, one round trip.** Full
continuous replication is rejected — the user looks at one thread at a time and tool payloads are
large — so only bodies are lazy while the summary list is fully mirrored.

Three cases are deliberately **proxied** rather than answered from the cache, because a cache that
answers them would be lying:

- **A version-6 open.** Only a client that asked for a window can be *told* the answer is cached;
  `AgentThreadSnapshot` has nowhere to say `synchronized = false`, so it is forwarded.
- **A cold mirror**, and a "load earlier" past what the prefix holds. The answer warms the mirror
  on the way back, so the next open is local.
- **A mirror so far behind its owner that the delta would trip the admission ladder.** Answering
  from a stale prefix that can never converge is worse than one round trip.

The **admission ladder** is the owner's: past `1 000` events or `8 MiB` in `(after_seq, head]`,
measured as `COUNT(*)` and `SUM(bytes)` over an indexed range and never as a payload read, the
owner answers with a **bounded window** instead of a giant replay. A cold mirror therefore asks
from sequence zero and lets the owner decide; past the ladder it gets a window with an empty tail
and stays cold, which is the bound that keeps a 50 000-event remote transcript out of this
daemon's database.

Version skew gates exactly one thing: the *request* sent upstream. The delta is asked for only of
a peer advertising `agent.window`, because a peer that ignores `after_seq` answers with a whole
projection — no events, nothing to cache. While such an older peer is connected, opens and any
event suffix after a mirror gap remain a plain proxy: the suffix is published unchanged and a
client repairs the gap through the legacy full open. Once the link is down, reading the durable
prefix needs no capability or Hello, which is what keeps the cached transcript readable offline.

Offline: **disconnection changes the status, never the data.** A mirrored transcript stays readable
from the prefix, the thread list stays painted from the durable mirror (the in-memory snapshot
fragment is the fallback, not the source), and every mutation fails as `ErrorKind::Remote` naming
the host.

### 9.4 Latency budget

Every number is a budget, which means it is a test. "Remote" is app ↔ local `fleetd` ↔ SSH ↔
remote `fleetd` at 40 ms RTT.

| Path | Local | Remote | Technique |
| --- | --- | --- | --- |
| Open a thread, first paint (warm) | **1 frame** | 1 frame | the retained projection is emitted synchronously |
| Open a thread, first paint (cold app, database warm) | ≤ 40 ms | ≤ 40 ms | header from `Snapshot::agent_threads` in frame 1; the windowed read hits the **local** mirror |
| Full window rendered | ≤ 80 ms | ≤ RTT + 80 ms | two-phase read (ids, then 25-row batches); row prep memoised by `RowKey` |
| "Load earlier" | ≤ 100 ms | ≤ RTT + 100 ms | keyset cursor; `LIMIT` genuinely bounds the scan |
| **Keystroke to echo** | **1 frame** | **1 frame** | the composer never awaits the daemon; nothing about a keystroke crosses a socket |
| Enter to the user's bubble | **1 frame** | **1 frame** | optimistic item with a client-generated id that *is* the server id |
| **Assistant token to pixel** | ≤ 16 ms + 200 ms + 1 frame | ≤ 16 ms + RTT + one batched local-mirror commit + 200 ms + 1 frame | 16 ms owner merge; one local mirror commit; UTF-8 reveal horizon; incremental Markdown; one row touched |
| Tool progress to pixel | ≤ 50 ms + 1 frame | + RTT | 50 ms last-wins |
| **Tool completion / turn end / gate open** | ≤ 1 frame | ≤ RTT + 1 frame | **zero added latency by rule** |
| Gate answer to the card clearing | ≤ 30 ms | ≤ RTT | optimistic clear, restored on transient failure |
| Interrupt to "Stopping…" | 1 frame | 1 frame | local state change on dispatch; the label holds until liveness clears |
| Tab strip repaint | ≤ 1 frame | ≤ 1 frame | denormalized counters on `threads` |
| Reconnect to `Live` (idle) | ≤ 60 ms | ≤ 2× RTT | `after_seq`: zero transcript bytes |
| Reconnect after 10 min | ≤ 200 ms | ≤ 2× RTT + 200 ms | the admission ladder trips and a **windowed** snapshot is sent instead of a giant replay |
| Daemon start to `AgentThreadList` | ≤ 50 ms @ 500 threads | — | one `SELECT`. **No log is read at start**; unprojected threads replay lazily |
| Writes on the frame path | **0** | **0** | the writer is a separate thread; the app never writes to disk |

The reading of that table is also the design rule: **the only things Fleet is willing to delay are
in-flight tool progress (50 ms) and the visual reveal of assistant text (at most 200 ms after its
16 ms merge). Completion, a turn end, a gate, and everything a human waits on have zero added
latency and flush queued prose first.**

## 10. Protocol additions (`fleet-proto`)

**`PROTOCOL_VERSION` is 8.** Windowing, seen cursors and the other independently negotiated verbs
below remain additive capabilities. The native-agent defaults change is not additive: version 7
requires `AgentThreadCreate.mode`, whereas version 8 omits it when the daemon should resolve the
per-harness default, and version 7 cannot decode the new `auto` and `dont_ask` permission values.
The exact-version Hello therefore rejects both mixed-version directions before either peer can
misdecode an agent request.

The version-6 agent request family survives in shape. What changes: `AgentThreadOpen` gains
`turn_limit`, `after_seq`, `before_cursor` and `request_sync_marker`, and answers with a
**bounded** `AgentThreadWindow { window, page, head_seq, seen_seq }` instead of an unbounded
`AgentThreadSnapshot`; `AgentItemBody` is added to page a large item body out of the store in
256 KiB chunks; `AgentMarkSeen` gains a real implementation backed by a `seen` table, replacing
the old validate-only path when a client identity is present; `Event::AgentWindow`,
`Event::AgentSynchronized` and `Event::AgentResync` are added.
`WINDOW_MAX_WIRE_BYTES = 2 MiB` is asserted on every window response against a fixture built from
the heaviest real transcript.

**The budget is respected by narrowing, never by enlarging the frame.** Over budget, the daemon
elides the largest item bodies to their head, names them in `TranscriptWindow.elided`, and only
then drops the oldest turns — the second lever, because losing a turn is visible where a collapsed
tool output is not. `fleet_proto::agents::elided_stream` is the **one** function that says which
of an item's append-only streams was cut, so the daemon's narrowing pass and the client's
`AgentItemBody` request cannot disagree about which body a page continues.

**Capability negotiation runs both ways.** `HelloClient` gains `capabilities`, by the same names
the daemon advertises in `HelloResponse.capabilities`. `Event` is adjacently tagged, so a variant
a peer has no arm for does not cost it one event — it fails the decode of the whole frame. The
three agent stream-control events therefore reach only a connection that named the capability
defining them (`agent.resync`, `agent.sync_marker`, `agent.window`); an older client keeps the
recovery it already had, which is a sequence gap in `Event::Agent`. `Event::Unknown` is never
re-broadcast: it is a tag *this* daemon could not name, so forwarding it tells no peer anything.

`agent.seen` gates persisted read cursors without a protocol-version bump. `HelloClient.client_id`
is the stable UUID created by the client library at `$FLEET_HOME/client-id`; a peer without it keeps
the historical validate-only `AgentMarkSeen` behaviour. A capable client asks once per negotiated
connection with `AgentSeenCursors` and receives its `Vec<AgentSeenCursor { thread, seq }>` census;
the app's health pass refreshes it after a transparent client reconnect. Every bounded
`AgentThreadWindow` also carries that connection's optional `seen_seq`. The census is deliberately
separate from `AgentThreadSummary`: summary broadcasts remain one shared O(1) fan-out, instead of
performing a per-connection database lookup for every summary event. Cursor upserts are monotonic.

`agent.closed` gates the per-installation closed-tab set without a protocol-version bump. A capable
client asks with `AgentClosedThreads` after Hello and receives `Vec<ThreadId>` before the first
snapshot. `AgentThreadClose` records the marker before routing, and `AgentThreadReopen` clears it
when the picker or top-level navigation restores the tab. An older client keeps ack-only close
behaviour. An older daemon leaves the app on its in-memory set.

Two additive fields close seams that used to be guesses rather than answers. `UserInput.item`
carries the identity the client already drew its optimistic bubble under, and the adapter adopts
it — Claude's announced `TurnStarted.user_item`, Codex's `clientUserMessageId` — so reconciliation
is by id instead of by message text plus an echo count. `ItemKind::UserMessage.steered` records
the harness's own answer to "did this join a turn that was already running?", so §7.2's leading
`↳` survives a reload instead of living only in the sending client's optimistic row.
`snapshot::AgentBinaries` gains `codex`, defaulted, so a host listing and `fleet doctor` report
the third executable the same way they report the other two (ADR 0014).

Two further metadata shapes were introduced additively in protocol 7.
`SessionConfigured.models` and `AgentSessionView.models` carry each model id, display name,
reasoning-effort ids and descriptions, and default effort; empty lists are omitted so the old
byte-exact goldens remain unchanged. `MetadataChanged.skills` carries a refreshed skill-name list
after `skills/changed`. Both replay through `ThreadProjection`, and neither invents vocabulary for
a provider that reported none.

`AgentRevert { thread, checkpoint }` restores a worktree from one Fleet checkpoint and answers
`AgentReverted` with what it put back and what it removed; `AgentCheckpoints { thread }` lists what
`[u]` may be drawn on. Both were additive in protocol 7 and gated on `agent.checkpoints`, because a
client that assumed checkpoints exist would draw `[u]` on every turn footer against a daemon that
cannot answer — and a daemon without the service refuses with a typed `Unsupported` rather than an
empty list, which would read as "nothing to revert to". `AgentRevert` joins the per-thread
serialization set for a stronger reason than ordering taste: it rewrites the worktree a running
turn is editing. `AgentCheckpoints` is a read and joins nothing.

`AgentAccountLogin { thread }` starts a harness sign-in and answers
`AgentAccountLogin { auth_url }`; `AgentAccountLogout { thread }` answers `AgentAck`. Both were
additive in protocol 7 and gated on `agent.account`. `RequestBody` is internally tagged with no
catch-all arm, so a daemon that predates these two variants cannot decode either frame at all;
the capability is how a peer asks before it sends. It says the *daemon* serves the verbs — whether
the **harness** on a given thread does is a second question, answered per thread with
`Unsupported`, which is why the composer offers `/login` on a Codex thread and on no other
(§7.1). Neither returns the account — that arrives as `AccountChanged`, so a mirror learns it
from the same event as the client that asked — and both are **refused on a mirror** rather than
forwarded, because the sign-in Codex starts is a loopback callback on the owner host and a
browser opened here would come back to the wrong machine. `AgentSessionView` gains `account`,
defaulted and omitted when absent, so a windowed open paints the chip on a cold thread.

The delegation wire family is **defined in phase 2 and served from phase 3**. It is gated by
`agent.delegation`; phase 2 defines the constant but deliberately does not add it to
`AGENT_CAPABILITIES`, so a client must not send these requests until a daemon advertises it.
Requests are internally tagged by `type`; their wire names are `delegation_run`,
`delegation_complete`, `delegation_list`, `delegation_get`, `delegation_cancel` and
`delegation_wait`, and field names are exactly the snake-case names below (`timeout_ms`, not
`timeoutMs`). Every `None` field, and every false boolean, is omitted from JSON:

```text
DelegationRun { caller, provider, brief, expectation,
    worktree?, mode?, model?, title?, fleet_path?, env={}, eager=false }
DelegationComplete { delegation, child, token, result, blocked=false }
DelegationList { caller? }
DelegationGet { delegation }
DelegationCancel { delegation }
DelegationWait { delegation, timeout_ms, caller? }
```

Three of those fields are additive and arrived after phase 2, each omitted from JSON when it is
absent or empty so an older peer's payload is byte-identical: `fleet_path` is the caller's
`fleet` directory hint (§15 preamble), `env` is the child environment map (§15 preamble) and
`caller` is the waiting thread that consumes a delivery (§15.2). None of the three bumped
`PROTOCOL_VERSION` or added a capability string.

`DelegationRun` answers `DelegationStarted { delegation, warning? }`.
`DelegationList` answers `Delegations(Vec<Delegation>)`; complete, get, cancel and wait each answer
`Delegation(Delegation)`, with wait returning either the terminal record or the current record at
its deadline. `Event::DelegationChanged(Delegation)` belongs to the `AgentSummary` subscription
family and is emitted only to peers that advertised `agent.delegation`. Run uses the harness-start
transport deadline; wait adds 15 seconds of transport slack to its `timeout_ms` service deadline.

Federation adds no agent-specific wire variant. Byte-exact goldens are added for **all** agent
requests, responses and events — `rust-ipc-protocol` Rule 9, and the current code has zero of
them. `request_timeout()` decisions are made per agent request; today they all default to 10 s
against a 30 s harness start.

The CLI drives the same requests: `fleet agent list|new|send|respond|interrupt|stop|tail`, with
`fleet agent terminal` for the PTY fallback.

## 11. UI kit additions

| Component | Contract |
| --- | --- |
| `TranscriptList` | Variable-height rows over GPUI `list`, bottom-aligned, keyed rows, overdraw, jump-to-latest, `^s [` scroll mode. Not `uniform_list`; ADR 0005's uniform-row rule applies to diffs, which stay uniform inside their row. Structural `set_rows` calls splice only for `diff_rows`; streaming `patch_row(index, row)` replaces one row and calls `remeasure_items`, preserving the reader's in-row scroll offset. `ListState::reset` remains exclusive to `set_thread`. The scroll callback consumes only `ListScrollEvent`; geometry and the scroll thumb are refreshed in a deferred entity update after GPUI releases the list's mutable lease. It reports reaching the oldest row it holds (`TranscriptEvent::ReachedOldest`, once per row set) and never decides whether older history exists — that is the owner's page cursor. |
| `MultilineInput` | Thin composer chrome over the shared multi-line `TextInput`: prompt history, owner-routed Enter submit, Shift-Enter newline, and `@` `/` `$` trigger reports. All editing, IME, selection, pointer geometry and scrolling stay in `TextInput`; history recalls at the *visual* buffer edge. |
| `Markdown` | Parses a *prefix* safely: every decided block stays a block, at the same index, as the stream grows. A table's header is the one two-line opener and stays the last paragraph until its delimiter arrives. `MarkdownDocument::append` reparses only that open tail and reuses its highlight cache. A partial fence is not highlighted and never reaches the cache. Prose is one wrapping `StyledText` per block; tables wrap equal-width cells; fenced code scrolls horizontally without capturing a vertical wheel. Images remain literal in v1. |
| `ToolRow` | The 30 px row: state glyph, kind column, summary, result, optional nested children region. Five states plus `Severe`. The click target is the 30 px line only, so clicking inside an expanded body does not fold the row, and the expand chevron is `invisible` rather than absent when a row cannot expand. |
| `DelegationRow` | Two lines: `↳`, provider glyph, title, the seven-state kit status word and its one mark, then the optional headline; elapsed time and the attach hint trail. It takes no domain type, is never grouped and stays exposed while live. |
| `DelegationResultCard` | Header `↳ <provider> finished · <word> · <elapsed> · <n> files` plus a Markdown body, collapsed to eight lines with the ordinary fold affordance and attach hint. |
| `DecisionDock` | The docked drawer: approval / question variants, amber left bar, keycap actions, `1/N` counter, owns key routing while open. Attaches to the composer by overlapping it and masking the shared border so the two read as one panel. |
| `MetadataRow` | A measured, ordered list of collapsible blocks; collapses from the right into an overflow menu. The hidden count is memoised **per width**, never recomputed per frame. A `MetadataSegment` may carry an opaque target: it renders in link tone with a focus ring and activates through click or `Enter`, which is how a child's pinned caller segment navigates without giving the kit a thread id. |
| `DiffView` | Lives in `fleet_lazygit::diff_view`, not the kit — it takes unified-diff text and keeps the ADR 0005 row stack. |
| `KeyHint` | Reused for every visible shortcut. |

All colours, sizes and durations come from tokens; no literal reaches a component and no domain
type enters the kit. `gallery_agent` is the example binary that shows every state, and it is the
acceptance test.

## 12. Keymap

The key context is `Agent` with sub-states, **derived from daemon state rather than from the
view** — that is what makes a decision own the keyboard in the same frame the gate appears rather
than one frame later. Precedence: `AgentNativeScroll` (a frozen tail beats everything, including
an open gate) > `AgentDecision > AgentPermission|AgentQuestion|AgentPlan` > `AgentWorking` >
`AgentIdle`. `^s` stays the only global prefix. `KEYMAP.md` § *Native agent thread* is
authoritative.

| Context | Keys |
| --- | --- |
| Idle | `⏎` send · `⇧⇥` plan mode · `↑` history · `^s m` model · `^s e` reasoning · `^s t` access mode · `^s [` scroll · `^s a`/`^s A` new Claude/Codex thread · `^s x` close tab · `^s F` terminal fallback |
| Working | `esc` interrupt · `⏎` send (a **steer**) · the same `^s` set |
| Permission | `y` allow once · `a` allow for this session · `n` deny · `e` edit (Claude only) · `esc` deny and stop. **`⏎` is not bound.** |
| Question | `1`–`9` choose · `space` toggle · `⏎` answer · `p` previous question |
| Plan | `y` implement · `n` refine · `⏎` expand |
| Scroll | `j`/`k` move focus · `ctrl-d`/`ctrl-u` · `gg`/`G` · `q`/`i`/`esc` leave |
| Row focus | `⏎` expand · `u` revert · `o` open in editor · `y` copy · `d` diff |
| Every sub-mode | the Workspace session rows: `^s s` hub · `^s S` sleep and hub · `^s h`/`p`, `^s l`/`n` previous/next tab · `^s 1`–`9`, `^s ⇥`, `^s w` select and MRU · `^s W` session switcher · `^s u` select the caller, attaching first · `^s d` open the `AGENTS` picker · `^s c` new terminal · `^s y` copy the worktree path · `^s z` zoom · `^s v`/`V`/`N`/`P` watch pane · `^s !` sticky error · `^s J` jobs · `^s ?` help · `^s esc` cancel |

On a focused delegation row, `⏎` attaches and selects the child and `x` cancels the delegation.
On a focused result card, `⏎` expands or collapses the body without attaching the child; `y`
copies the delegation id from either row. `^s x` closes a caller tab but only detaches a child,
and `^s u` returns from a child to its caller with the caller's composer focused.

**A thread tab repeats the Workspace session table, minus its PTY rows.** The thread is the
Workspace's selected tab, so `Agent > …` *replaces* `Workspace > …` rather than covering it and
nothing is inherited; without the repeat every session command was a dead key in an agent tab.
`^s r` (restart), `^s ,` (rename), `^s ]` (paste) and `^s ^s` (send a literal) stay out, because
each addresses a PTY and a Fleet-drawn tab has none. No handler moved: `^s s` is already on the
`Fleet` root and the rest on the Workspace root, both ancestors of the agent view.

**`^s` is taken by the shell's keystroke interceptor, not by gpui's two-key matcher.** gpui
replays the keystrokes of a sequence that matched nothing as *input*
(`Window::replay_pending_input`), so a tab drawn around a text composer had `^s s` type an `s`
into the draft. The interceptor consumes `^s`, resolves the second key against the *live* context
chain through `keymap::chord_action_for_chain`, and where no row names it consumes that key too
and toasts `^s <key> is not bound here`. It is the same one-shot shape `Workspace > Prefix`
already has, and it leaves no pending input armed inside gpui.

Three more rules come with the table. **Row focus lives inside scroll mode**, which finally makes
`⏎`/`u`/`o` fire and retires the long-standing caveat that `Agent > AgentRow` is bound, handled and
never entered. **`e` on a permission, `n` on a plan and a free-text answer all *open* the composer
rather than answering at once**, and while that draft is being typed the gate's context stands
down so the text may start with a `y`. And **gpui's `>` is a subsequence test, not a parent test**,
so every `^s` row is registered against each sub-mode by name — the product lives in
`keymap::SHARED_TABLES` rather than as a copied block per context — and an embedded pane may not
reuse any context word this table uses. There is a test for each.

The `esc` cascade, in order: close a picker → abandon a gate draft → leave scroll mode →
interrupt, but only when something is running. On an idle thread with nothing open, `esc` does
nothing. `esc` never quits.

## 13. Status

The shipped implementation described by the previous revision targeted Claude Code + OpenCode and
has been replaced. This table is the plan and its progress. **Every "owed" line below is a thing
this build does not do**, named here rather than softened in the section that specifies it.
`TODO.md` at the repo root restates them with a starting file, a cost, and a done-when for each.

| # | Phase | Status |
| --- | --- | --- |
| 1 | **Domain.** New item/event model in `fleet-core::agents`, indexed projection replacing the O(n²) scans, `projection.rs` split under ~900 lines, `should_apply_lifecycle` | **done**; `providers/opencode/**` was deleted with it. `ItemKind::UserMessage` later gained `steered` and `UserInput` gained `item`, both additive and both with a producer |
| 2 | **Storage.** `rusqlite`, the schema, the owned writer thread, the read pool, the migration ladder, `FleetHome::agents_*`, the one-shot NDJSON import, the one-`SELECT` thread list, lazy hydration and the background boot repair | **done**. The `seen` table has a monotonic owned-writer upsert and bounded per-install census. Owed: `item_attachments` has no writer because attachments are not built |
| 3 | **Harnesses.** The `Harness` trait and probe; Claude stream-json; Codex app-server; scripted fixtures | **done** against Claude 2.1.275 and Codex 0.154.0. Claude discovers `initialize.models` (with `list_models` and a static fallback), publishes per-model effort ladders, and restarts with resume for every model/effort/mode change. Codex discovers every `model/list` page, sends launch effort through `config.model_reasoning_effort`, and updates effort plus permissions in place. Both adapters publish their supported mode lists. The manager reads the adapter's `Submitted::JoinedActive { turn }` versus `Submitted::QueuedNew { turn }` answer, so a steer is distinguishable from a queued turn. **Owed**: the per-thread raw NDJSON log, Codex cold rehydration from its own store, and per-instance homes (multi-account is out of scope per §14). |
| 4 | **Protocol.** Windowed open, `AgentItemBody`, the sync/resync events, real `AgentMarkSeen`, durable closed tabs, byte-exact goldens, per-request timeouts, the capability strings | **done**. Ten `agent.*` capabilities are advertised and served. `agent.delegation` gates the six delegation requests and `DelegationChanged`; `agent.account` gates `AgentAccountLogin`/`AgentAccountLogout`. `agent.seen` added `HelloClient.client_id`, `AgentSeenCursors`, `AgentThreadWindow.seen_seq`, and a monotonic store write additively in protocol 7; anonymous peers retain validate-only compatibility. `agent.closed` additively gates `AgentClosedThreads` and `AgentThreadReopen` without changing protocol 8. The app seeds its cursor and closed sets after every Hello, so reconnecting neither restores a cleared amber dot nor a closed tab. Protocol 8 remains the exact-version boundary for defaulted agent creates and the expanded permission enum |
| 5 | **Transcript.** The flat row model, the eighteen row kinds, `TranscriptList`, `ToolRow`, the fold and group logic, the scroll machine, `gallery_agent` | **done**. Kit (5a): the flat `TranscriptRow`, all eighteen kinds, `TranscriptList` over `list` with the three-state scroll machine and its generation counter, the six-state `ToolRow`, the group summarizer, the streaming-safe `Markdown` with its highlight cache, `gallery_agent`. Screen (5b): `screens/agent_thread/rows/` projects a thread into those rows — the §B1.4 emission order, the fold exemption table, the live-activity tail walk and its present-tense rule, the group summarizer's inputs, and the settled-gate record — memoised behind a `RowsKey` so a stream chunk rewrites one row and re-runs no grouping. Scroll-back paging is closed end to end: `TranscriptEvent::ReachedOldest` reports the gesture, the workspace asks the mirror for a page cursor, and `merge_older_page` prepends the answer. Deferred scroll refresh no longer borrows list state from inside its own callback; the two-turn `scroll-wheel.scenario` exercises wheel input up and back at a 600-pixel viewport |
| 6 | **Decisions and controls.** `DecisionDock`, the three gate kinds, the composer, the control cluster and pickers, `MetadataRow` overflow | **done**. Kit (5a): `DecisionDock` with its attachment seam, the `Decision` priority ladder and key vocabulary, `MetadataRow` with its per-width fit memo, `MultilineInput`'s three trigger reports. Screen (6): `/login` and `/logout` join the `/` built-ins on a Codex thread and nowhere else, and the metadata row's last trailing segment is the account — `signed out`, or the email, or the plan, or nothing at all; the docked drawer wired to daemon state so a gate owns the keyboard in the same frame, `⏎` unbound on a permission, the question wizard with per-question drafts, the plan verbs on the composer, `ComposerMode`'s capability table, the three control tiers with the restart rule, six completion surfaces, provider-described Codex effort rows and refreshed `$` skills, and the §12 key contexts including row focus inside scroll mode. The harness projects a prepared decision on each thread as `{kind,title,paths,has_diff}`, and the regular corpus proves a Codex file approval joins its exact item. **Not built**: attachments (nothing uploads one, so `--add-dir` is not granted either — granting a directory nothing can put a file in is an affordance with no behaviour behind it) and the `$`-to-`/` skill rewrite (the daemon's adapter boundary owns it) |
| 7 | **Remote.** The mirror column and its authority rules, snapshot-then-delta, the admission ladder | **done**: `store/mirror.rs` owns the `owner_host` columns and the only statements that write them, `manager/mirror.rs` the read-through cache, `router/agents.rs` the `AgentMirror` seam the link hangs on, and `manager/window.rs` the windowed open and the admission ladder. All four authority rules have a test. The app sends window fields on every open, so the warm-mirror path is reachable from the UI. **Owed**: the SQL-native window read of spec-C C.2.5 — the window's *content* still comes from the reducer's projection, so a windowed open of a cold thread replays its log once — and `mirror_oldest_seq` stays `NULL` because the mirror only ever stores prefixes from sequence 1 |
| 8 | **Checkpoints and revert.** Fleet-owned git refs, `AgentRevert`, `[u]` | **done**: `services/checkpoints/` captures a turn or a file scope into `refs/fleet/checkpoints/`, reverts a worktree from one without touching `HEAD`, the index or the conversation, and garbage collects per thread plus an hourly orphan sweep. `AgentSessionManager` holds the service and takes both captures — `capture_turn` in `send`, for a turn that is actually starting rather than a steer, and `capture_files` on the `ItemStarted` of an edit-shaped tool. A capture failure logs and the turn proceeds, always (§5). The app draws `[u] revert turn` from `AgentCheckpoints` and sends `AgentRevert`. **Owed**: `[u] revert this edit` on a tool row. A file-scope checkpoint names the *turn* it was taken in and not the item, so a tool row has nothing to key on; and the capture is best-effort by construction, because neither harness waits for Fleet before running an auto-approved tool — the turn-scope checkpoint is the guarantee, the file-scope one is the finer-grained revert when the race goes Fleet's way, which it always does for a gated edit |
| 9 | **Delegations.** Durable caller/child model, transcript origin, storage migration, capability-gated wire family, transcript rows, attach/detach navigation, `AGENTS` picker and restart recovery | **done**. A child starts hidden, attaches from its durable row or `^s d`, detaches without stopping, bubbles attention to its caller, and delivers one result card. Startup resumes one provider exit, preserves exact-once delivery, repairs deleted callers, and cancellation walks descendants first |

The whole Workspace session table — selection and MRU (`^s 1`–`9`, `^s Tab`, `^s w`), `^s s`,
`^s S`, `^s h`/`p`/`l`/`n`, `^s W`, `^s u`, `^s d`, `^s c`, `^s y`, `^s z`,
`^s v`/`V`/`N`/`P`, `^s !`, `^s J`,
`^s ?` — is bound inside every agent-tab sub-mode, minus the four PTY-only rows, and an unbound
second key is swallowed with a toast rather than typed into the composer (§12). The
fixture-driven `scenarios/agents/` corpus is the GUI smoke pass for the agent tab.

One gap belongs to the seam between the app and the daemon rather than to either side:

- **A resolved gate's record is window-local.** The projection holds only the *open* gates, so the
  settled `Gate` row is built from what this window watched close and answered. A client that
  reconnects has the open gates and none of the closed ones, and the row it drew for an answered
  permission is gone. The `gates` table already stores the resolved row — status, answer, resolver,
  `resolved_seq` — so closing it is a `resolved_gates` vector on `ThreadProjection` plus a field on
  `TranscriptWindow`, not new storage. It is named here rather than approximated, because an
  invented outcome on a decision the user made is the one kind of wrong a transcript must not be.

The optimistic user bubble **is** reconciled by id: `UserInput.item` carries the client's own
`ItemId`, Claude announces it as `TurnStarted.user_item`, and Codex sends it as
`clientUserMessageId` then echoes it as `userMessage.item.clientId`, so the row the user saw and
the row the daemon recorded are one item. Both Codex lifecycle frames are suppressed after the
join. For Codex builds that omit `clientId`, the adapter consumes the first unreconciled pending
user item with identical text; that compatibility fallback is submission-ordered and remains
inside the adapter rather than becoming transcript identity.

## 14. Risks and open questions

- **Protocol drift, both harnesses.** Neither wire is a documented public contract, and Codex
  demonstrably adds enum values without a version bump. Current mitigation is §4.5's tolerant
  decoding, the capability gates, per-version fixtures and the terminal fallback. The per-thread
  raw NDJSON log remains deferred in `TODO.md` §7 — every nuance in §4 was discovered by reading
  one, which is why that missing diagnostic matters.
- **Claude in-session control requests are unproven.** t3code gets `setModel`/`setPermissionMode`
  from the SDK; the corresponding `control_request` subtypes are not proven by any capture Fleet
  holds. Shipping a guess means a silently-ignored control, so v1 treats every such change as
  `RestartWithResume`, executed only at a turn boundary. When a capture proves the subtypes, the
  adapter flips one `ControlCost` to `InPlace` and nothing above the trait changes — which is
  exactly why `apply_runtime` returns `RuntimeApplied` rather than `()`.
- **A restart per Build/Plan toggle on Claude is a visible latency** the UI must not pretend away.
- **A Claude `--resume` may trigger the CLI's own resume-compaction prompt**, which Fleet answers
  like any other question, defaulting to "keep full history".
- **Codex interrupt scope.** Stop fans out to children first with per-child deadlines; a wedged
  child must not block the parent forever.
- **Markdown scope.** A focused in-house renderer was chosen over Zed's `markdown` crate to
  respect ADR 0003 and keep the dependency graph small (ADR 0010). GFM pipe tables are in scope;
  images and indented code remain literal source text.
- **Per-edit revert is deferred.** Turn checkpoints are built, but a file-scope checkpoint does
  not yet carry the `ItemId` a tool row needs to offer `u` truthfully (`TODO.md` §4).
- **Attachments are deferred.** The wire can describe them, but no composer, store writer or
  per-thread attachment directory produces one, so Fleet does not grant Claude `--add-dir`
  (`TODO.md` §5).
- **A cold window still replays once.** Windowed responses are bounded, but their content comes
  from the reducer projection rather than the planned SQL-native window read (`TODO.md` §6).
- **Multi-account shadow homes** (t3code's symlink overlay) are out of scope. If Fleet ever wants
  two Codex accounts sharing thread continuity, the non-obvious part is that the continuation key
  must ignore the shadow home. **Single-account sign-in and sign-out are in scope and built**
  (§4.2, §7.1): `/login` and `/logout` drive the one account of the `CODEX_HOME` the thread runs
  under, which is a different thing from running two accounts side by side. Per-instance harness
  homes are correspondingly deferred even though `HarnessConfig.home` is plumbed (`TODO.md` §8).
- **An MCP wrapper is deferred.** The CLI remains the one delegation contract; a later MCP server
  may wrap its six verbs without introducing a second state machine (`TODO.md` §9).
- **Terminal caller discovery is deferred.** `fleet subagent run --caller <thread>` works, but a
  terminal user must discover and copy that id themselves (`TODO.md` §10).
- **Remote-host delegations are deferred.** A mirrored caller is refused rather than creating a
  local child whose durable record would be split from its authoritative transcript (`TODO.md`
  §11).
- **Structured delegation results are deferred.** `--json-result` validates its input, but the
  stored and delivered contract remains bounded text (`TODO.md` §12).
- **Caller-authorized gate answers through the CLI are deferred.** The first CLI controls the
  delegation lifecycle; it does not expose a general child-thread control surface (`TODO.md`
  §13).
- **A Jobs-screen delegation projection is deferred.** Delegations remain visible in their caller
  transcript and keep their own lifecycle; a later Jobs view may be read-only (`TODO.md` §14).
- **Human PR review is not an agent state.** It stays in Hub/Pull Requests.

## 15. Delegations

A delegation is **two durable things**, never a special harness mode. The child is an ordinary
native-agent thread owned by `AgentSessionManager`, with its caller in `parent` and its delegation
id on the thread record. Beside it is a `Delegation` record: why the child exists, which caller
turn and transcript item launched it, where its answer must go, its lifecycle, result and delivery
state. The thread owns conversation and provider lifecycle; the record owns the caller/child
contract. Rebuilding a thread never deletes or recreates that record.

`fleet subagent run` is accepted only during a running caller turn. It creates the child with
`FLEET_DELEGATION=<id>` and `FLEET_DELEGATION_TOKEN=<token>`, inserts the delegation row, appends
an `ItemKind::Delegation` under that turn, then sends the first message. The default worktree is
the caller's. An omitted mode resolves through the configured default for the selected harness
(which is `full_access` when unset); `--mode` accepts `ask`, `accept-edits`, `plan`, `auto`,
`dont-ask`, and `full-access`. `--model` and `--effort` are independent: `--effort` may be passed
without `--model`, in which case the child keeps the provider's configured default model
(`config.nativeAgents.<provider>.model`) and gets the requested effort. That pairing travels as a
`ModelSelection` whose `model` is **the empty string** — the sentinel documented on the field for
"keep the configured default" — which `create_with` fills from the defaults; when the provider has
no configured model either, the selection reaches the adapter still empty and the adapter names no
model at all while still spending the effort. A blank `--model ""` remains a validation error: the
flag was typed, so reading it as the default would hide a quoting mistake. Fleet never validates
the effort string — the legal ladder is per provider and per model (§7.1), so a bad value is the
provider's error to report. The default child title is
`↳ <provider> — <first line of the brief, cut at 48 characters>`. A caller on a remote mirror is
refused: phase 3 runs children only on the daemon that owns the caller.

**The child gets a `fleet` on its `PATH`.** A child that cannot run `fleet subagent complete` can
never report, so the daemon resolves one directory and prepends it to the child's environment,
in this order:

1. the directory of the `fleet` the caller itself ran — `fleet subagent run` puts its own
   canonicalised `std::env::current_exe()` on the request — **when that exact file also exists on
   the daemon's host**. This is the same-host case, and the only one where wire compatibility is
   guaranteed;
2. otherwise a `fleet` sitting next to this daemon's own `fleetd`, which is what `make build` and
   a remote bootstrap both leave behind;
3. otherwise nothing: the child keeps whatever its login shell's `PATH` holds, exactly as before.

The hint is advisory. It describes the caller's host, so it may be absent, stale or name a path
the daemon does not have, and every one of those degrades to the next rule rather than refusing
the delegation. The prepend **extends** `PATH` rather than replacing it — it is carried as
`StartRequest::path_prepend`, not as an `env` entry, because both adapters treat an `env` entry as
a whole-value override and a `PATH` there would discard the login shell's own. Prepending is
idempotent. The daemon logs which directory it injected and which rule chose it at `info`, and
warns when no rule matched. `fleet doctor` reports the same daemon-side directory (§"subagent
fleet CLI").

**A resumed child keeps it.** Restarting a thread or resuming it lazily rebuilds its
`StartRequest` from the durable record, and there is no caller request there to read a hint from,
so rule 1 is unavailable — the hint describes a process that has already exited and is
deliberately not persisted. Rule 2 does not depend on a request, so the resume path re-runs it for
any thread that has a delegation, and logs the outcome the same way. A resumed thread with no
delegation is injected nothing, as before: only a child is expected to report. The practical
consequence is that a child recovered after a provider exit can still run the
`fleet subagent complete` the recovery nudge asks it for, even though the directory it gets may
differ from the one it was first started with.

**The caller may hand the child environment variables.** `fleet subagent run --env KEY=VALUE` is
repeatable, reaches the daemon as `RequestBody::DelegationRun.env`, and is merged into the child's
environment **before** Fleet's own identity variables, so `FLEET_DELEGATION` and
`FLEET_DELEGATION_TOKEN` always win. The daemon enforces that order itself rather than trusting the
CLI to have done it: any peer can send the field. The CLI refuses five things, each with a
validation error naming what it rejected: a value with no `=`, an empty key, a key given twice —
silently keeping the last would hide a typo — any key beginning `FLEET_`, and `PATH`. `PATH` is refused because both adapters
treat an `env` entry as a **whole-value override**, so one here would discard the login shell's own
rather than extend it; extending is what the `path_prepend` above is for. The motivating case is
several children sharing one worktree: giving each its own `CARGO_TARGET_DIR` is what stops them
serialising on a single cargo build lock. Fleet suggests nothing of the sort on its own — the
delegation footer stays generic and cargo advice belongs in the orchestrator's brief.

**A resumed child keeps its environment too.** The map is persisted beside the delegation
(`delegations.env_json`, migration slot 6) and restored on resume ahead of the freshly rotated
identity variables, with the same precedence and for the same reason. Without that, a child
recovered after a provider exit would silently lose its `CARGO_TARGET_DIR` and rejoin the build-lock
fight in the one situation where nobody is watching. A delegation row written before slot 6 reads as
an empty environment. The map is deliberately **not** a field on `Delegation`: it is never put on
the wire, never rendered by the CLI, never echoed in a `--json` envelope and never logged, because a
user variable may hold a secret and because echoing it back would be exactly the context bloat the
elided brief exists to stop. It lives in the same database as the delegation token hash.

### 15.1 State and completion

The lifecycle is:

```text
Starting --SessionConfigured--> Running
any live state --GateOpened---> Blocked --GateResolved/GateWithdrawn--> Running
completed + reported + no background work ----------------------------> Succeeded
completed + reported + background work --> Settling --clear/deadline--> Succeeded
completed + no report + nudge remaining -> Settling + Nudge
Starting | Running | Blocked | Settling --terminal ending------------->
    Succeeded | Incomplete | Failed | Cancelled
```

An already-terminal delegation ignores every later child event. `headline` may change on a child
tool starting, a terminal item update/completion, or turn settlement; streaming `ContentDelta`
never writes it.

"Done" has three independent parts, and their arrival order is not significant:

1. The child reports a result through `fleet subagent complete` with the right delegation id,
   child thread and bearer token.
2. Its turn settles `Completed` with no open question, plan or permission gate.
3. Its projected background-task set is empty. If a reported child settles while background work
   remains, it stays `Settling` until that set clears or `created + 30 seconds` has passed, then
   succeeds even if that work never emits its terminal event.

A report may beat settlement or settlement may beat the report. An identical second report is an
idempotent success; a different second report is refused, as are a wrong token, a wrong child and
any report against a terminal delegation. `--blocked` stores the report, sets `Blocked` with
`status_payload = "reported blocked"`, and becomes `Failed` when the turn settles.

A child whose completed turn has no reported result is nudged, at most twice. Each later completed
turn re-evaluates the same rule. After the second nudge is exhausted, the next completed turn ends
`Incomplete`, using the first 4 KiB of the latest assistant message as a
`LastAssistantText` result. Every settlement refreshes that fallback unless a `Reported` result
already exists; a reported result always wins.

| Child ending | Delegation ending | Result and follow-up |
| --- | --- | --- |
| `TurnSettled(Completed)`, reported, no background work | `Succeeded` | deliver the reported result |
| `TurnSettled(Completed)`, reported, background work live | `Settling`, then `Succeeded` | wait for background work or the 30 s grace, then deliver |
| `TurnSettled(Completed)`, no report, nudges remain | `Settling` | send the nudge and continue |
| `TurnSettled(Completed)`, no report, nudges exhausted | `Incomplete` | capture the latest assistant text and deliver |
| `TurnSettled(Completed)` after `--blocked` | `Failed` | deliver the blocked report |
| `TurnSettled(Error | MaxTurns | BudgetExhausted | Denied | Other)` | `Failed` | retain the outcome and latest assistant text, then deliver |
| `TurnSettled(Interrupted)` | `Cancelled` | deliver and cancel live descendants depth-first |
| `TurnAborted(User | SessionStopped | Timeout | Superseded | Other)` | `Cancelled` | deliver and queue cancellation of live descendants |
| first `TurnAborted(ProviderExited)` | keep `Running` or `Blocked` | set `recoveries = 1`, enqueue `Recover`, resume with the recovery nudge |
| later `TurnAborted(ProviderExited)` | `Failed` | payload `provider exited twice`, then deliver |
| fatal `RuntimeError` or unexpected `SessionExited` | `Failed` | deliver |
| expected `SessionExited` while live | `Cancelled` | deliver and cancel live descendants depth-first |

`fleet subagent cancel` refuses an already-terminal record, cancels every live descendant
depth-first, then interrupts and stops the child. Each resulting `TurnAborted(SessionStopped)`
takes the ordinary `Cancelled` delivery path. A Stop issued through any other surface enqueues
the same descendant propagation before the child is considered finished.

### 15.2 Delivery and exactly once

Every follow-up action caused by a state change (`Mirror`, `Nudge`, `Settle`, `Deliver`, `Recover`
and `CancelChildren`) enters the delegation outbox in the same SQLite transaction as
the child or caller event that caused it. The worker drains once before serving, on every wake,
and every 60 seconds. It handles `CancelChildren` rows first, in id order, so a deferred delivery
cannot starve cancellation; those propagation rows are exempt from the per-caller throttle. It
then handles every other row in id order, at most one per caller per pass. Two children finishing
together therefore become two caller turns rather than one combined turn. A row stays open until
its action actually happens, so a daemon restart or transient error costs a retry, not a lost
result.

Delivery first terminally patches the caller's delegation transcript item, completing it as
`Completed` for `Succeeded` and `Failed` for every other terminal status. It then chooses by the
caller's durable state:

| Caller state | Delivery rule |
| --- | --- |
| `Ready`, idle, no open gate | start a new turn with a user message whose origin is `Delegation { id }` |
| running, `eager == false` | leave `Deliver` open until the turn settles |
| running, `eager == true` | send now; the harness steers the active turn |
| stopped by `ProviderExit`, with a resume cursor | send, allowing the manager to resume the caller |
| stopped by the user | leave `Deliver` open until a later `SessionConfigured` |
| blocked on a gate | leave `Deliver` open until the gate resolves or withdraws |
| starting, or parked in provider `Waiting` | leave `Deliver` open until the session can accept input |
| stopped with no resume cursor, or record missing | set `Undeliverable { reason }` and close the row |

Both shipped adapters mint a cursor at start, so the cursorless stopped-caller case is a legacy-record path.

A send that loses a race with a newly-running turn returns `Conflict` and leaves the row open; any
other send failure is logged and retried. The worker deliberately does **not** mark `Deliver` done
after calling `send`. When the caller's `ItemStarted(UserMessage { origin: Delegation { id } })`
is committed, `project_event` sets `delivery = Delivered { seq, turn }` and marks that exact
outbox row done in the same transaction. This caller-item rule is the exactly-once boundary: the
message's durable identity, not a successful function return, proves delivery.

**A caller that already took the result consumes the delivery.** `fleet subagent wait` names the
waiting thread on the wire as `DelegationWait.caller`. When the wait resolves a **terminal** record
whose `caller` is exactly that thread, the same write sets `delivery = Consumed`, marks the open
`Deliver` row done, and publishes the changed record so the app repaints; the wait then returns the
record it just changed, so the very response that consumed the delivery already reads
`"delivery": "consumed"`. The worker afterwards still terminally patches the caller's delegation
transcript item — the row has to stop saying "working" — and still closes its outbox row, but it
sends **no** user message, and logs the skip once at `info` naming the delegation and the caller.

`caller` is advisory *identity*, never authorisation: it decides whether a delivery is consumed,
never whether the wait is answered. A `wait` that carries no caller — an older `fleet`, or one typed
in a shell with no `FLEET_SESSION` — consumes nothing, and neither does a `wait` from any thread
other than the delegation's own caller; both are answered in full and both still get the ordinary
delivered user message. Consuming is best effort and idempotent: a record already `Delivered`,
`Undeliverable` or `Consumed` is left exactly as it is and the wait answers anyway, so a `wait` that
races the delivery worker and loses simply sees `delivered`, which is correct rather than an error.
The guarantee is unchanged in strength and only sharper in wording: **a result reaches its caller at
most once, by whichever of `wait` and the delivery worker gets there first.** The motivating failure
was an orchestrator that waited on eight children and then, when its turn settled, received all
eight results a second time as user messages.

### 15.3 Limits and bearer token

- Delegation depth is at most 3; a caller at depth 3 cannot spawn another child.
- One caller may have at most 4 live children, and one daemon at most 8 live delegations.
- A child receives at most 2 missing-result nudges.
- `SETTLE_GRACE` is 30 seconds; the retry tick is 60 seconds.
- Results are capped at `ITEM_BODY_MAX_CHUNK_BYTES` (256 KiB). The cap applies **at ingest**: the
  tail above it is discarded when `complete` stores the report and nothing anywhere keeps it.
  Truncation sets `elided` and is named in CLI stderr, in the delivered message and wherever the
  report is rendered afterwards. Below the cap the stored report is the whole report, and both
  `fleet subagent wait` and `fleet subagent status` return it in full — on the wire and in their
  human output. There is no second verb, no `--full` flag and no separate fetch: a caller that
  wants the body reads it from either of those two, never from a file the child happened to leave
  behind.
- `fleet subagent wait` defaults to 540 seconds, chosen to sit under the Claude Code shell-tool
  ceiling, and imposes **no upper bound of its own**: a larger `--timeout` is accepted, though the
  caller's own tool timeout may still kill the wait. A wait that reaches its timeout exits 2 and
  prints a distinct non-terminal line naming the delegation, its current status and the elapsed
  wait; it never claims a running child finished. A terminal record exits 0, and the child's
  report body is returned on success in both human output and the JSON envelope's `result.text`.
  `--json` output is identical either way. `wait` also names the waiting thread — `--caller`, else
  `FLEET_SESSION` — so a caller waiting on its own child consumes the delivery (§15.2); the flag is
  never required, and a wait with no caller behaves exactly as it always did.
- `fleet subagent status` and `fleet subagent list` report the child's token usage, its dollar cost
  when the provider reported one, and its context percentage. The numbers are the child's **own
  thread only, descendants excluded** — a delegation tree is never summed — and they are its settled
  turns plus the in-flight turn's latest report, which is the same definition the app's turn footer
  uses, computed from the same recorded values. They are computed on read, so they are never
  persisted on the delegation record and never carried by `DelegationChanged`; a child that has
  reported no usage at all renders `-` rather than a zero. `fleet agent list` is unchanged: showing
  usage there needs a `threads`-table migration and a projector change, and that is deferred.
- `fleet agent tail <THREAD>` follows a child's event stream. `--no-follow` prints the retained
  snapshot and exits 0 without entering the follow loop, and `--last <N>` trims that snapshot to
  the last N events; both imply `--replay`, because a tail that printed nothing is the bug they
  exist to fix. Neither adds paginated history to the protocol — the trim is client-side, over
  what the cursored open already returned.

The token is two concatenated `Uuid::new_v4().simple()` values: 64 lowercase hexadecimal
characters, or 32 random bytes. Only its SHA-256 hex digest is stored. `complete` hashes the
presented token and compares the two digests with a constant-time XOR fold. The plaintext exists
only in the child's `FLEET_DELEGATION_TOKEN`; the delegation id is separately available as
`FLEET_DELEGATION`, and `FLEET_SESSION` identifies the child thread.

### 15.4 Exact child and caller copy

The child's first message is its brief, one blank line, then this footer with `{id}`,
`{expectation}` and `{fleet}` substituted:

```text
--- Fleet delegation {id} ---
You are running as a subagent. No human is watching this session.
The caller expects: {expectation}
When the work is fully finished and verified, report it with exactly one command:
  {fleet} subagent complete --result-file <path-to-your-report.md>
Write the report first, then run the command. Do not run it before you are done.
If you are blocked and cannot finish, run:
  {fleet} subagent complete --blocked --result-file <path-with-what-you-need>
Do not ask the user questions; state assumptions in the report instead.
```

`{fleet}` is the **absolute path** of the executable rules 1 and 2 above resolved, shell-quoted so
a Fleet installed under a path with a space still yields a runnable command line:

```text
  /Users/you/fleetd/target/debug/fleet subagent complete --result-file <path-to-your-report.md>
```

It is the literal `fleet` only when rule 3 applied and no path is known. Naming the path is not
redundant with the `PATH` prepend: it is the one surface where a name that does not resolve costs
the entire delegation, so the child is given both.

The missing-result nudge is exactly:

```text
You have not reported a result. If the work is done, run `fleet subagent complete --result-file <path>`. If not, continue.
```

The recovery nudge is exactly:

```text
The session was restarted. Continue, and report with `fleet subagent complete` when done.
```

When the child inherits the caller's worktree **by default** — that is, when `--worktree` was
omitted — `run` returns this warning:

```text
no --worktree was passed, so the child edits the caller's worktree by default; end your turn before it works, or pass --worktree to isolate it
```

A caller that passed `--worktree` gets **no** warning, even when the worktree it named is the
caller's own. Naming it is a decision, not an accident: an orchestrator that hands its children
disjoint file ownership inside one worktree does exactly this, and warning it every time would
train the warning out of being read. Only the implicit default is warned about. The warning is
appended to human output and carried in the JSON `warning` field.

The delivered message is:

```text
[fleet subagent <id> finished: succeeded]
provider: codex, thread: <child>, duration: 14m 02s, files changed: 6

<text>
```

The status word is `succeeded`, `incomplete`, `failed` or `cancelled`. An elided result adds one
blank line and `(report elided at <n> bytes)`.

A `fleet subagent wait` that reaches its timeout prints one line and nothing else, and exits 2:

```text
[fleet subagent <id> still running after 9m 47s, status: running, thread: <child>]
```

It is deliberately not the delivered message's shape: the bracketed prefix matches so a caller
can scan for it, but there is no `finished:` and no body, because there is nothing to report yet.
Its status word is the delegation-status name (`starting`, `running`, `blocked`, …) rather than
the transcript row vocabulary, for the same reason the delivered line's is.

`fleet subagent list` prints one fixed-field tab-separated line per delegation, now **eight** fields
rather than six — id, status, provider, child, duration, total tokens, cost, delivery — with `-` in
the tokens and cost fields when the child has reported no usage. Nothing else about the line moved;
the two new fields sit after `duration` and before `delivery`.

`fleet subagent status` is no longer that same row. It prints, in order: the fixed-field line, the
brief, the child's usage, and — for a terminal delegation — the report, rendered through the exact
delivered-message template above, elision suffix included. Rendering it through that one template is
the point: a caller that greps `wait`'s output and a caller that greps `status`'s are reading the
same bytes for the same record. The usage line names total tokens, input and output, cache reads and
writes, `context N%`, and `$X.XX` when a cost was reported; a child with no usage prints no line at
all rather than a row of zeros. `fleet subagent cancel` is untouched — its human output remains the
single word `cancelled`, with no brief, no usage and no report body.

`run`, `wait` and `list` **elide the brief from their `--json` envelopes**, replacing a brief longer
than 200 characters with its first 200 on a character boundary and setting `briefElided: true`
beside it. The flag is truthful rather than a marker of which verb answered: a brief of 200
characters or fewer is carried whole and the key is omitted entirely, not set to `false`, so a
caller may trust `delegation.brief` whenever it does not see the flag. A brief is
written by the orchestrator, so echoing a 250-line one back costs it 250 lines of its own context to
learn nothing. The elision is a rendering rule in the CLI: the wire still carries the whole brief,
and `status` (with `cancel`, which shares its envelope) still prints it whole, which is where a
caller goes when it genuinely wants to read a brief back. Human output is unchanged everywhere.

### 15.5 UI: rows, attachment and attention

The caller transcript projects `ItemKind::Delegation` as a `DelegationRow` joined to the durable
record. It shows the provider, child title, status word and mark, latest headline, elapsed time and
`attach`; a live row never folds into a completed-work group. Each delivered
`UserMessage { origin: Delegation { id } }` produces exactly one `DelegationResultCard`, not a
user bubble: its header names the provider, terminal word, elapsed time and changed-file count,
and its Markdown
body collapses to eight lines. Expanding that card does not attach the child.

A new child starts hidden. `Enter` on its delegation row attaches and selects the same
daemon-owned thread, gives its composer focus, and adds `↳ <provider> — <title>` to the mixed
strip immediately after its caller and older attached siblings. `Enter` on the delivered result
card expands it while the child stays hidden. `^s x` on an attached child detaches it without
stopping it; the caller's durable row remains and can attach it again. The
child's pinned metadata begins `for [<n>] <provider> — <title>` (`·` replaces the index when the
caller is hidden), and activating that segment or `^s u` attaches and selects the caller. Its
composer says `Steering a subagent of [<n>]. It reports to its caller when it finishes.`

Child attention folds into the caller so a hidden child is not silent:

| Child/caller fact | Caller presentation |
| --- | --- |
| the caller itself has an open permission, question or plan gate | keep the caller's own `needs you`; its priority remains above every child working contribution |
| any child has an open permission, question or plan gate | `needs you`; the highest-priority open gate wins and remains while the caller is selected |
| otherwise a child is working/waiting, or the caller itself is working | `working`; a live child can outrank the caller's failed, finished, unread or idle state, and the family is included once in the context count under the caller |
| a child is finished, idle or otherwise no longer live | contributes nothing; the caller's own attention remains |

`^s d` opens the palette seeded to its `AGENTS` section. Rows show an attached strip index or `·`,
the attention mark, the caller or `↳` child title, status and age (or the open gate), and `go` or
`attach`. The order is current-worktree callers, their children, then their other-worktree
children with a ` · <worktree>` suffix. `Enter` selects an attached thread, attaches a hidden
child, reopens a closed caller, or switches worktrees before attaching a remote-worktree child;
each path focuses the selected composer. Nothing auto-attaches merely because it is blocked.
Closing a selected caller removes its tab, selects the remaining terminal and returns the
Workspace to `TERMINAL`; the picker keeps that caller as a `·` / `go` / hidden row until it is
reopened.
Because the strip stops at nine tabs, attach what you look at, detach when done, and reach the
rest through `^s d`.

### 15.6 Restart and recovery

Startup has four ordered boundaries:

1. Before it reads delegation outbox rows, the worker hydrates every live child. Hydration runs
   the ordinary orphan pass, which records `TurnAborted(ProviderExited)` for a child whose
   provider disappeared with the daemon.
2. That committed transition applies the resume-once rule. With `recoveries == 0`, it preserves
   `Running` or `Blocked`, writes `recoveries = 1` and opens one `Recover` row; a later provider
   exit ends `Failed` with `status_payload = "provider exited twice"` and opens `Deliver`.
3. The worker begins its startup outbox drain. As the drain's preflight — and before every later
   drain as well — it repairs terminal, pending delegations whose caller record was deleted to
   `Undeliverable { reason: "caller deleted" }`, preserving their result and closing their open
   delivery row.
4. It then performs cancellation propagation first and reads the remaining open rows in id order,
   at most one non-cancellation row per caller in that pass. An open delivery from the previous
   daemon therefore drains on restart, while the durable caller-message origin remains the
   exact-once boundary and prevents a duplicate item on later restarts.

`Recover` sends the exact recovery nudge from §15.4 through the child's resume cursor. A child
without a usable cursor ends `Failed` with that reason and is delivered instead. A successful
recovery starts one resumed turn, may finish
`Succeeded`, and retains `recoveries = 1`; the recovery nudge and the caller's delegation-origin
message each occur exactly once, and a completed drain leaves no open row. A second provider exit
never gets another nudge: its `provider exited twice` failure survives another restart and is
delivered exactly once. Resuming once can repeat work the provider performed before its last
durable event; that is the accepted cost of continuing instead of failing on the first exit.

Cancellation has the same durability. Explicit cancellation walks the bounded delegation tree
depth-first, so a grandchild reaches `Cancelled` and records its `Deliver` row before its parent;
both delivery rows remain recorded. A Stop or other cancelling child event writes
`CancelChildren` beside its own `Deliver`, so a restart cannot lose propagation. A descendant's
delivery may become `Undeliverable` if its caller is stopped before that caller drains it, but the
state and result remain on the record.
