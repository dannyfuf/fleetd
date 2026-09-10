# Native agents

Authority over: how Fleet runs Claude Code and Codex as structured sessions, the provider-neutral
event model, the thread state machine, the transcript's row vocabulary, the decision surfaces, the
control cluster, and the SQLite store that makes all of it feel instant. `ARCHITECTURE.md`
("Native agent sessions"), `APP-CONTRACTS.md` (§3, §4, §5), `UX-SPEC.md` (§3.6.0), `KEYMAP.md`
("Native agent thread"), `DESIGN-SYSTEM.md` (§6.6), `REMOTE-MACHINES.md` (§7) and ADRs
`decisions/0010-native-agents.md`, `0013-sqlite-agent-transcripts.md`,
`0014-drop-opencode-add-codex.md` carry the seams that touch them; where one of those disagrees
with this file about its own surface, that file wins.

**This document describes a system that is not built.** §13 is the status table and every row in
it says so. The previous revision of this file described a shipped Claude Code + OpenCode
implementation; that implementation is being replaced, OpenCode is being deleted, and §13 records
what survives.

Sources this was distilled from:

- **The installed harnesses, captured live** on 2026-09-10: `claude 2.1.266` and
  `codex-cli 0.147.0`. `research/harness-protocols.md` and
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
  thread — §8 keeps every thread browsable, so the daemon goes on listing one this window has
  closed and the strip is what forgets it. No thread-list sidebar, no inspector, no detached diff
  pane. Wide windows leave the right side empty on purpose; the content measure is 760 px with a
  16 px inset.
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
remote link. `raw` is a **pointer** into the per-thread NDJSON debug log, never an inline payload:
inlining raw frames is what makes a snapshot undecodable at 16 MiB.

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
view:     last_seen_seq            // per client, persisted daemon-side (new)
```

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
   shows a spinner on a finished turn.
4. Gates are independent of turns. A gate closes only on `GateResolved` or `GateWithdrawn`.
5. Stream or process loss is `SessionExited { expected: false }` plus `RuntimeError`, which makes
   the turn `Failed`. Never inferred success.
6. A user message sent while running is **steering**, dispatched immediately. There is no queue
   and no `QueuedMessage` row — see §7.2.
7. Attention derives from gates, then work, then failure, then fresh completion.
8. **Settlement is sticky.** `Interrupted` never downgrades to `Completed`; `Failed` never
   downgrades. This is what makes "the user pressed Stop and the turn completed anyway" render
   correctly.

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

- **`--effort` is real in 2.1.266.** The old claim that Claude has no reasoning ladder — and the
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
(t3code `ClaudeHome.ts:118-127`).

Handshake is `system/init`. Fleet reads six fields: `session_id` (**the resume cursor**, persisted
before any turn), `capabilities` (the version gate), `model`, `permissionMode` (advisory), the
`tools`/`slash_commands`/`skills`/`agents`/`mcp_servers` vocabularies for the composer's `/`, `@`
and `$` menus — **per session, discovered here, never hardcoded** — and `claude_code_version` for
the log.

Frames not in the previous revision, all observed live: `system/status` (a truthful spinner
sub-label, never a terminal), `system/thinking_tokens` (a live reasoning-token counter that lets
the collapsed row tick `thinking · 132 tokens` with the body closed), `rate_limit_event` (two
utilisation windows, §4.1.2), and `system/permission_denied` (above — **must render**; dropping it
makes a refusal look like a hang).

Streaming: `stream_event` deltas move the cursor, `assistant` frames backfill completed blocks —
**one frame per content block, several frames sharing one `message.id`**, so the *content block*
is the transcript atom, not the message. Four invariants: `signature_delta` is **dropped** (it
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

Of 99 client methods Fleet uses fifteen: `initialize`, `thread/start`, `thread/resume`,
`thread/read`, `thread/items/list`, `turn/start`, `turn/steer`, `turn/interrupt`,
`thread/settings/update`, `thread/compact/start`, `thread/fork`, `thread/unsubscribe`,
`model/list` (**cursor-paginated — loop on `nextCursor`**), `skills/list`, and the two account
reads. Each carries a Fleet-side deadline, because the protocol has none, and each deadline is
strictly less than the `fleet-proto` timeout of the client call that triggered it, so the daemon
always answers before the client gives up.

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

### 4.3 Divergence — where normalisation would lose something

The full 48-row table is in the harness spec; these are the rows where flattening the two
harnesses into one would destroy something the user must still see. Fleet carries the difference
into the UI rather than pretending it away.

| Axis | Claude Code | Codex | Fleet |
| --- | --- | --- | --- |
| "Always allow" scope | real, via `updatedPermissions` with a persistent destination | **not expressible** for exec/file approvals; survives only in MCP elicitation `_meta.persist` | the button is offered on Claude and **absent** on Codex. Advertising a button that silently means something weaker is the worse outcome. |
| Auto-approval by the harness | — | `approvalsReviewer: "auto_review"`, a risk-assessing subagent, plus `item/autoApprovalReview/*` rows | Fleet's "Auto" mode means **materially different things**. The mode picker's help text differs per harness. Collapsing this would be a safety misrepresentation. |
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
| Session ready | `system/init` | `initialize` result **and** `thread/start`/`resume` result | process spawn succeeding |
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

**A version gate is never a `PROTOCOL_VERSION` bump.** Remote links require an exact protocol
match, so a new harness capability is an additive capability string on the existing handshake.
Adding Codex must not make a mixed-version fleet unable to connect.

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

### 4.6 Fallback

`spawn` fails with `HarnessError::Unavailable { reason }` and the reason is user-facing copy, not a
stack trace: binary not on PATH, installed but failed to run, handshake timeout, signed out (with
JSON-quoted paths so they survive any shell), or too old for Fleet. Every one names the same
escape hatch: **`^s F` opens the agent in a terminal on the same worktree**. The native tab does
not half-work; it says why and points at the fallback.

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

Update policy: a `ContentDelta` mutates only the text of the row for that item and notifies;
structural events rebuild grouping. Turn metadata is **withheld until the turn completes**, so the
footer never moves under the reader.

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

`[u] revert this edit` and `[u] revert turn` are **not built**. The design is Fleet-owned
checkpoints — a hidden git ref of the worktree before each turn and per file before each edit —
deliberately decoupled from harness completion and from harness conversation rewind. `[u]` is
drawn only where checkpoints exist; a drawn affordance that does nothing is worse than an absent
one (`DESIGN-SYSTEM.md` §7).

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

| Control | Claude | Codex | Mid-thread? | Mid-turn? | Key |
| --- | --- | --- | --- | --- | --- |
| Model | launch flag; any selection change restarts with the resume cursor | sent on each `turn/start`; no restart | yes | no | `^s m` |
| Reasoning effort | `--effort`; restarts with resume cursor | `effort` on `turn/start`; no restart. A non-empty **string**, not an enum | yes | no | `^s e` |
| Context window | a `select` that **rewrites the model id** via a suffix (`claude-opus-5[1m]`); also the meter's denominator | not offered | yes | no | `^s e` |
| Fast mode / service tier | a setting; restarts | `serviceTier`, a `select`; no restart | yes | no | `^s e` |
| `ultrathink` | **not an effort** — a prompt prefix, skipped when the prompt starts with a slash command | n/a | yes | no | `^s e` |
| Access mode | one `--permission-mode` | three axes: `approvalPolicy` × `sandbox` × reviewer. **The reviewer is re-sent every time**, or `auto_review` stays sticky | yes — restarts on both | no | `^s t` |
| Build ⇄ Plan | `--permission-mode plan`, restoring the **base** mode on leaving | a `collaborationMode` block on `turn/start` | yes — **no restart on either** | no | `⇧⇥` |
| Compaction | a slash command: send `/compact` as a turn, and synthesise the boundary if the turn settles without one | native `thread/compact/start` | yes | no | — |
| Instance (same driver) | restarts with resume cursor | same | yes | no | `^s m` |
| Instance (different driver) | **rejected** — a Claude thread cannot become a Codex thread | same | never | never | — |

**Three orthogonal axes Fleet must not collapse into one "mode":** approval policy, sandbox
policy, permission profile. Claude has one permission mode plus a rule list. Fleet's four-mode
ladder is a *presentation* over Codex's three axes, not a replacement, and note that "Auto"
differs from "Auto-accept edits" *only* by the reviewer — `auto_review` is Codex running its own
risk-assessing subagent in the user's place, which has no Claude equivalent (§4.3).

**Nothing applies mid-turn.** Every control writes to a local draft and takes effect at the next
send; a picker that blocks on a round trip stops feeling instant over an SSH link. Control state
lives in three tiers: the composer draft (app, per thread, persisted — writing is synchronous and
repaints one row), a sticky per-instance memory (app, global), and the thread projection (daemon,
persisted). Before restart work begins, `session = Starting` is published optimistically so the
tab shows `starting…` in the same frame rather than after the round trip.

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
`item_attachments`, and `fleet_migrations`. No foreign keys — deletes are explicit multi-table
statements in the projector, which is what you want when you also have to delete files.

Retention: no compaction, no `VACUUM`, no prefix delete. Deletion happens per **thread**, never
per sequence prefix, because a prefix delete invalidates the meaning of a projector cursor.

`state.sqlite` is **not** part of `PersistedState`. It gets its own migration ladder and its own
failure mode: a database the daemon cannot open or migrate is fatal at start, exactly as an
unreadable `state.json` is, and for the same reason — the daemon must not run with half a truth.

Four disciplines survive from `store.rs` and must be reproduced: a header-versioned log, torn-tail
quarantine rather than data loss, selective durability, and restart-recovery-as-appended-events. A
thread the log leaves `Starting` or `Running` is an orphan and is settled **explicitly, as
appended events**, never as a silent state edit.

## 9. The live path, the client mirror, and remote

### 9.1 Coalescing

Two windows on the broadcast path, and no others: **16 ms merge** for `ContentDelta` (one frame,
free latency) and **50 ms last-wins** for `ItemUpdated`. Every other event — a tool completing, a
turn settling, a gate opening — **flushes the pending window and emits with zero added latency, by
rule**, asserted by a test that the clock does not advance. Assistant text is merged, never
delayed further.

Backpressure: a budget overflow emits `Event::AgentResync` and the client re-opens with
`after_seq`. Never a stall, never an OOM, **never a silent drop**.

### 9.2 The client mirror

The app keeps **no on-disk cache**; its warm tier is a 5-minute in-memory map and the daemon's
SQLite is the cache. On open it paints the retained `Arc<ThreadProjection>` synchronously, and a
`Live` status stays `Live` so no label flashes. Optimistic sends use a **client-generated `ItemId`
that *is* the server id**, so reconciliation is by id with no temp-id swap and no matching
heuristic. "Sending" clears on a **field diff** against a pre-send snapshot — any server-visible
movement clears it — not on a correlated ack, which a *steer* would never produce.

### 9.3 Remote

> **The remote daemon owns the transcript and the sequence. The local daemon is a re-framing proxy
> that also maintains a durable read-through mirror in its own `state.sqlite`. The app never knows
> whether a thread is local or remote.**

The existing routing model is correct and tested (`tests/agents_remote.rs`) and survives
unchanged. The **no-copy rule does not**: today `AgentThreadOpen` ships the whole
`ThreadProjection` over SSH against a 16 MiB ceiling with no compression, and past that ceiling it
becomes permanently *undecodable* — a codec error, not a truncation.

A mirrored thread is stored in the **same tables** as a local one with one nullable
`threads.owner_host` set, because a mirrored thread is a prefix of the owner's log with the
owner's own `seq` values. That buys the property that makes remote feel local: **`AgentThreadOpen`
runs the same SQL whether the thread is local or remote.** One snapshot query, one cursor codec,
one row model, one code path in the app.

The mirror is a read-through cache, **never a replica**: never consulted for a write, never merged
field-by-field, and any disagreement resolves in favour of the owner. Four authority rules, each
with a test: a thread with `owner_host` set may only be appended to from that host's link (two
writers on one sequence space is silent corruption); no harness process is ever started for a
mirrored thread; a mutation routes upstream and is never applied locally on optimism; and the
mirror never fabricates a `Synchronized`.

Warm mirror, remote thread: the local daemon answers **entirely from the mirror in ~1 ms** with
`synchronized = false`, the app paints the full transcript as `Cached`, and in parallel the local
daemon asks the owner for `(after_seq, head]`. **Zero transcript bytes cross the link, one round
trip.** Full continuous replication is rejected — the user looks at one thread at a time and tool
payloads are large — so only bodies are lazy while the summary list is fully mirrored.

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
| **Assistant token to pixel** | ≤ 16 ms + 1 frame | ≤ 16 ms + RTT + 1 frame | 16 ms merge, appended in SQL, one row touched |
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
in-flight tool progress (50 ms) and text that has not finished a frame (16 ms). Everything a human
reads word by word or waits on is zero added latency.**

## 10. Protocol additions (`fleet-proto`)

**`PROTOCOL_VERSION` stays at 7.** Everything is additive and gated on a capability string. A bump
would force every remote daemon to upgrade in lockstep — the daemon matches the version literally
and remote links require an exact match — and the entire point of the capability mechanism is to
avoid that.

The version-6 agent request family survives in shape. What changes: `AgentThreadOpen` gains
`turn_limit`, `after_seq` and `request_sync_marker`, and answers with a **bounded**
`AgentThreadWindow { window, page, head_seq }` instead of an unbounded `AgentThreadSnapshot`;
`AgentItemBody` is added to page a large tool output out of the log in 256 KiB chunks;
`AgentMarkSeen` gains a real implementation backed by a `seen` table, replacing today's no-op;
`Event::AgentWindow`, `Event::AgentSynchronized` and `Event::AgentResync` are added.
`WINDOW_MAX_WIRE_BYTES = 2 MiB` is asserted on every window response against a fixture built from
the heaviest real transcript.

`AgentRevert` is **not** in the protocol: the checkpoint service it would address does not exist.

Federation adds no agent-specific wire variant. Byte-exact goldens are added for **all** agent
requests, responses and events — `rust-ipc-protocol` Rule 9, and the current code has zero of
them. `request_timeout()` decisions are made per agent request; today they all default to 10 s
against a 30 s harness start.

The CLI drives the same requests: `fleet agent list|new|send|respond|interrupt|stop|tail`, with
`fleet agent terminal` for the PTY fallback.

## 11. UI kit additions

| Component | Contract |
| --- | --- |
| `TranscriptList` | Variable-height rows over GPUI `list`, bottom-aligned, keyed rows, overdraw, jump-to-latest, `^s [` scroll mode. Not `uniform_list`; ADR 0005's uniform-row rule applies to diffs, which stay uniform inside their row. |
| `MultilineInput` | Wrapping, IME, paste, history, Enter/Shift+Enter, `@` `/` `$` triggers. Reports triggers rather than consuming the keys, so all three stay typable. |
| `Markdown` | Parses a *prefix* safely: every decided block stays a block, at the same index, as the stream grows. No tables or images in v1. |
| `ToolRow` | The 30 px row: state glyph, kind column, summary, result, optional nested children region. The click target is the 30 px line only, so clicking inside an expanded body does not fold the row. |
| `DecisionDock` | The docked drawer: approval / question variants, amber left bar, keycap actions, `1/N` counter, owns key routing while open. Attaches to the composer by overlapping it and masking the shared border so the two read as one panel. |
| `MetadataRow` | A measured, ordered list of collapsible blocks; collapses from the right into an overflow menu. The hidden count is memoised **per width**, never recomputed per frame. |
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

Three rules come with the table. **Row focus lives inside scroll mode**, which finally makes
`⏎`/`u`/`o` fire and retires the long-standing caveat that `Agent > AgentRow` is bound, handled and
never entered. **`e` on a permission, `n` on a plan and a free-text answer all *open* the composer
rather than answering at once**, and while that draft is being typed the gate's context stands
down so the text may start with a `y`. And **gpui's `>` is a subsequence test, not a parent test**,
so the `^s` rows must be repeated on every `AgentDecision > *` context and an embedded pane may
not reuse any context word this table uses — there is a test for it.

The `esc` cascade, in order: close a picker → abandon a gate draft → leave scroll mode →
interrupt, but only when something is running. On an idle thread with nothing open, `esc` does
nothing. `esc` never quits.

## 13. Status

**Nothing in this document is built.** The shipped implementation described by the previous
revision targets Claude Code + OpenCode and is being replaced. This table is the plan.

| # | Phase | Status |
| --- | --- | --- |
| 1 | **Domain.** New item/event model in `fleet-core::agents`, indexed projection replacing the O(n²) scans, `projection.rs` split under ~900 lines, `should_apply_lifecycle`, Codex fixtures under `research/fixtures/agents/codex/` | **not started** |
| 2 | **Storage.** `rusqlite`, the schema, the owned writer thread, the read pool, the migration ladder, `FleetHome::agents_*`, the one-shot NDJSON import | **not started** |
| 3 | **Harnesses.** The `Harness` trait and probe; the Claude adapter rewritten against the 2.1.266 wire; the Codex adapter over app-server with generated `wire.rs`/`methods.rs`; **`providers/opencode/**` deleted** | **not started** |
| 4 | **Protocol.** Windowed open, `AgentItemBody`, the sync/resync events, real `AgentMarkSeen`, byte-exact goldens, per-request timeouts, the capability string | **not started** |
| 5 | **Transcript.** The flat row model, the eighteen row kinds, `TranscriptList`, `ToolRow`, the fold and group logic, the scroll machine, `gallery_agent` | **not started** |
| 6 | **Decisions and controls.** `DecisionDock`, the three gate kinds, the composer, the control cluster and pickers, `MetadataRow` overflow | **not started** |
| 7 | **Remote.** The mirror column and its authority rules, snapshot-then-delta, the admission ladder | **not started** |
| 8 | **Checkpoints and revert.** Fleet-owned git refs, `AgentRevert`, `[u]` | **not started** |

Carried forward as known gaps, each to be named where the user meets it: turn checkpoints and
revert (`[u]` says so rather than doing nothing); the Workspace prefix inside an agent tab (only
`^s m e t [ a A x F` are bound); and a GUI smoke pass for the agent tab, which ADR 0007's driver
script still lacks.

## 14. Risks and open questions

- **Protocol drift, both harnesses.** Neither wire is a documented public contract, and Codex
  demonstrably adds enum values without a version bump. Mitigation is §4.5's tolerant decoding, the
  capability gates, per-version fixtures, the terminal fallback, and a per-thread raw NDJSON log
  behind a flag from day one — every nuance in §4 was discovered by reading one.
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
  respect ADR 0003 and keep the dependency graph small (ADR 0010); tables and images come later.
- **Multi-account shadow homes** (t3code's symlink overlay) are out of scope. If Fleet ever wants
  two Codex accounts sharing thread continuity, the non-obvious part is that the continuation key
  must ignore the shadow home.
- **Human PR review is not an agent state.** It stays in Hub/Pull Requests.
