# Native agents

Authority over: how Fleet runs Claude Code and OpenCode as structured sessions, the
provider-neutral event model, the thread state machine, and the native agent tab in
`fleet-app`. This document is the design **and** what shipped; §10 is the status table that says
which parts are done and which are follow-ups. `ARCHITECTURE.md` ("Native agent sessions"),
`APP-CONTRACTS.md` (§3, §4, §5), `UX-SPEC.md` (§3.6.0), `KEYMAP.md` ("Native agent thread"),
`DESIGN-SYSTEM.md` (§6.6) and ADR `decisions/0010-native-agents.md` carry the seams that touch
them; where one of those disagrees with this file about its own surface, that file wins.

Sources this was distilled from:

- The **Fleet Redesign** canvas, artboards *Agent tab — working*, *Agent tab — needs you*,
  *Agent thread — message vocabulary*, *Signals*, *Workspace*, *Terminal chrome*.
- **t3code** (`pingdotgg/t3code` @ `8b2838e`), a desktop control surface for the same harnesses.
  Its server owns harness processes, normalises every protocol into one event type, persists an
  append-only event log, and derives "needs attention" from several orthogonal signals. Fleet
  copies that ownership boundary and its completion rules, not its stack.
- The current Fleet code: daemon-owned PTY sessions, framed JSON protocol, the app bridge,
  `fleet-lazygit` as the precedent for a native per-worktree entity, `fleet-ui-kit` primitives.

## 1. Goal

Replace the agent terminal (`AgentPopup` over a PTY running `claude` or `opencode`) with a
native GPUI conversation that Fleet understands: it knows when a turn is running, when the
agent is blocked on a permission, a question or a plan, when a turn finished, and when a
process died. The terminal path stays as an explicit fallback, now on `^s F`.

Non-goals for the first release: Codex app-server, ACP agents, remote hosts, a thread sidebar,
voice, and a full editor as the composer.

## 2. Product shape

The canvas fixes these decisions; do not relitigate them in code.

- **Agents are workspace tabs, not a separate screen.** A Claude or OpenCode session is a
  numbered tab in the same strip as terminals: `[2] claude — rounding fix`,
  `[6] opencode — tz shifts`. `^s a` starts Claude, `^s A` starts OpenCode, `^s 1–9` selects,
  `^s x` closes — the *tab*, not the thread: §6 keeps every thread browsable, so the daemon goes
  on listing one this window has closed and the strip is what forgets it.
  No thread-list sidebar, no inspector, no detached diff pane. Wide windows leave
  the right side empty on purpose; the content measure is 760 px with a 16 px inset.
  The title after the em dash is the thread title. OpenCode publishes one (`Session.title`) and
  it replaces the fallback as soon as it arrives; Claude's stream-json carries no title on any
  frame, so a Claude tab keeps the fallback — the first user message, normalised and cut at 48
  characters — for the life of the thread. The two therefore read differently on a long first
  prompt, and that is the provider's limit, not a rendering choice.
- **The pane is a transcript above a docked composer.** The transcript is bottom-anchored.
  The composer is a 36 px input (`❯` prompt glyph, placeholder `Message claude… (@ file ·
  / command)`), Enter sends, Shift+Enter inserts a newline, no send button, and a 22 px
  metadata row: `agent mode · claude-sonnet-5 · high · asks before edits` on the left,
  `context 34% · $0.42 · 48m` on the right. Every left-hand segment the provider reports is
  shown and no segment is invented: the reasoning level appears once one is known — OpenCode
  names it on the model reference and `^s m` sets it on either provider — while Claude's
  `system/init` publishes a model and no effort, so a fresh Claude tab has three segments rather
  than four. The model itself is seeded at session start on both providers (OpenCode reads its
  configured default from `GET /config`), so the row is complete before the first turn; the
  right-hand side is turn-derived and stays empty until one has run.
- **One visual vocabulary for both providers.** User turns are the only block with a
  background (`#16181D`, radius 6). Assistant prose sits on the ground with no bubble, avatar
  or header. Thinking is one collapsed muted line (`thinking · 6s  [⏎] show`). Every tool call
  is one 30 px row: `state glyph · 60 px kind column · one-line summary · right-aligned result`.
  Completed work folds to `worked 12s · 3 tool calls  [⏎] show`. A turn ends with one
  right-aligned footer: `48s · 12.4k tokens · 2 files changed +36 −3 · [⏎] diff · [u] revert turn`.
  The duration is the turn's own: the minutes it stood parked on a decision card are the user's,
  not the model's, so the reducer takes each gate's open window back off the provider's figure.
- **Decisions are cards in the thread, never modals.** Permission, question and plan cards
  share one shape: 760 px, `#16181D`, radius 6, a 2 px amber bar on the left, always the last
  item. While a card is open the composer stays visible at 60 % opacity and bare keys route to
  the card; the status bar mirrors the card's keys.
- **Colour is semantic.** Green = alive/fine, amber = needs you or cannot verify, red =
  broken, gray = everything else including progress, blue = where you are and never a state.
  Only gray spinners and the text cursor animate. Attention is a static amber dot or bar.
- **Where state shows.** Tab: gray spinner (running), amber dot (needs you), gray dot (unread
  output), `exited 1` in red (dead). The amber dot is the one mark that survives selection —
  a thread blocked on you is blocked whether or not you are reading it, while an unread dot on
  the open tab would be news you have already read. Session header: `working` or `needs you`.
  Context bar: `3 needs you · 2 working · 1 failed`, which includes the current tab and omits
  every segment whose count is zero.

Exact copy, keys and dimensions are in the canvas; `UX-SPEC.md` §3.6.0 carries the agent tab
section, and `DESIGN-SYSTEM.md` §2.8 carries the six fixed dimensions as
`components::agent::metrics` constants.

## 3. Architecture

Fleet's rule holds: the daemon owns truth, clients consume snapshots and events, rendering
never blocks on IO. Agent sessions therefore live in the daemon and survive app restarts, like
terminals do today, and additionally survive daemon restarts through persisted transcripts and
provider resume cursors.

```
fleet-daemon
  services/agents/
    manager.rs        AgentSessionManager: threads, lifecycle, sequencing, attention
    thread.rs         one thread's runtime: serialized operation gate, delta coalescing
    store.rs          per-thread append-only event log + versioned thread index
    providers/
      mod.rs          trait AgentProvider, ProviderError, spawn_provider
      claude/         Claude Code over bidirectional stream-json (stdio): wire, map, process
      opencode/       OpenCode over HTTP + SSE (managed `opencode serve` per thread)
fleet-core
  agents/             ids, AgentEvent, items, gates, state enums, projection (the reducer)
fleet-proto
  agent requests / responses / events / snapshot summaries (versioned, seq-numbered)
fleet-client
  api/agents.rs       typed commands; api/agents/mirror.rs applies ordered deltas, resync on gap
fleet-app
  screens/agent_thread/   Entity<AgentThreadView>: rows, decisions, presentation, picker
  screens/workspace/agent.rs   tab routing and the BridgeCommand relay
  state/agents.rs         summaries, projections, seen cursors, attention edges
fleet-ui-kit
  components/agent/       TranscriptList, ToolRow, DecisionCard, format, metrics
  components/{markdown,multiline_input}
fleet-lazygit
  diff_view.rs            the reusable inline DiffView (kept out of the kit; see ADR 0010)
```

Ownership boundary, copied from t3code: **provider IO produces normalised events; a
serialised reducer persists them and updates projections; views render projections.** GPUI
never owns lifecycle truth. Provider wire types never leave `fleet-daemon`.

### 3.1 Provider adapter trait

```rust
#[async_trait]
pub trait AgentProvider: Send {
    fn kind(&self) -> AgentKind;                      // Claude | OpenCode
    fn capabilities(&self) -> Capabilities;           // resume, fork, steer, interrupt, modes, models
    async fn start(&mut self, req: StartRequest) -> ProviderResult<()>;   // fresh or resume(cursor)
    async fn send(&mut self, turn: TurnId, input: UserInput) -> ProviderResult<()>;
    async fn interrupt(&mut self, turn: TurnId) -> ProviderResult<()>;
    async fn respond(&mut self, gate: GateId, answer: GateAnswer) -> ProviderResult<()>;
    async fn set_mode(&mut self, mode: PermissionMode) -> ProviderResult<()>;
    async fn set_model(&mut self, model: ModelSelection) -> ProviderResult<()>;
    async fn stop(&mut self) -> ProviderResult<()>;
    fn events(&mut self) -> ProviderEvents;           // mpsc::Receiver<AgentEvent>
}
```

`ProviderError` is `Unavailable { reason } | Protocol { message } | Exited { code } |
Timeout { what }`; `Unavailable` is the typed answer a missing or too-old executable produces,
and it names the `^s F` terminal fallback. That fallback is *same-worktree*: pressed on an agent
tab it ensures a session named `<worktree session>/agent-<agent>` whose PTY runs the configured
agent command in the worktree path, so the CLI opens on the very tree the thread is editing.
The Hub's `^a`/`^o` keep the one repository-level session (`swarm-agent-claude`,
`swarm-agent-opencode`) rooted at `repos_dir`. `spawn_provider(kind, &StartRequest,
&AgentCommands)` is the factory, so the executables stay the ones `config.agentCommands`
already named for the popup. Each adapter is responsible for its own completion authority,
permission mapping, resume behaviour and cancellation budgets. The manager treats them
identically.

### 3.2 Normalised event model (`fleet-core::agents`)

Identities: `ThreadId`, `TurnId`, `ItemId`, `GateId`, plus a per-thread monotonically increasing
`seq: u64` stamped by the reducer, a timestamp, and `raw: Option<String>` carrying the provider
type name for debugging and the `[⏎] raw` action.

```rust
pub enum AgentEvent {
    // session
    SessionStarted { provider: AgentKind, resume_cursor: Option<String>, model, mode, tools, commands, skills },
    SessionStateChanged(SessionState),
    SessionExited { code: Option<i32>, expected: bool },
    // turn
    TurnStarted { turn: TurnId, user_item: ItemId },
    TurnCompleted { turn: TurnId, outcome: TurnOutcome, usage: Usage, duration_ms, files_changed: Vec<FileDelta> },
    TurnAborted { turn: TurnId, reason: AbortReason },
    // items
    ItemStarted { turn: TurnId, item: ItemId, kind: ItemKind, parent: Option<ItemId> },
    ContentDelta { item: ItemId, stream: StreamKind, delta: String },   // assistant_text | reasoning | tool_output
    ItemUpdated { item: ItemId, patch: ItemPatch },                     // tool summary, result, diff, status
    ItemCompleted { item: ItemId, status: ItemStatus },                 // Done | Error | Denied
    // gates (human-in-the-loop), independent of turn state
    GateOpened { gate: GateId, turn: Option<TurnId>, kind: GateKind },  // Permission | Question | Plan
    GateResolved { gate: GateId, answer: GateAnswer, by: GateResolver },
    // observability
    TokenUsage { turn: TurnId, usage: Usage, context_pct: f32, cost_usd: Option<f64> },
    Checkpoint(CheckpointKind),                                          // CompactBoundary{before,after} | Resumed{age}
    Retrying { attempt: u32, retry_in_ms: u64, reason: String },
    RuntimeError { fatal: bool, message: String },
    Notice(String),                                                      // config warnings, deprecations
}
```

`ItemKind`: `UserMessage { text, attachments }`, `AssistantText`, `Thinking`, `Tool { kind:
ToolKind, name: String, input: Value }`, `Subagent { name, description }`, `Error`. `ToolKind`
is the design's kind column: `Read | Edit | Write | Bash | Search | Grep | Fetch | Agent | Todo |
Skill | Mcp { server } | Unknown`. Provider-native tool names map to it in the adapter (Claude
`Glob`→Search, `WebFetch`→Fetch, `Task`→Agent, `TodoWrite`→Todo, OpenCode `bash`→Bash,
`edit`/`write`→Edit/Write, and so on). Unknown tools show their raw name.

`GateKind`:

```rust
Permission { tool: ToolKind, title: String, payload: String, rationale: Option<String>,
             options: Vec<PermissionOption> }   // AllowOnce | AllowSession | Deny | Edit | DenyAndStop
Question   { questions: Vec<Question> }         // 1–4 questions, 2–4 options each, multi_select, "other" text
Plan       { markdown: String, steps: Vec<String> }   // Approve | AskForChanges | ViewFull
```

Provider-native option ids are retained inside `PermissionOption` so the answer maps back
exactly (Claude `allow` + `updatedPermissions`, OpenCode `once`/`always`/`reject`); `payload` is
the invocation itself, never the model's prose description of it, because it is what the card
shows literally and what `e` seeds the composer with. Copy always
spells out the effective scope: `allow once`, `allow for this session` (Claude), `allow for this
directory` (OpenCode), never the word "always".

### 3.3 Thread state: orthogonal axes, not one enum

```
session:  Starting | Ready | Running | Stopped | Error
turn:     None | Running(TurnId) | Completed(TurnId, outcome) | Interrupted(TurnId) | Failed(TurnId)
gates:    Vec<OpenGate>            // permissions, questions, actionable plan
work:     open_items, background_tasks (subagents), retrying
view:     last_seen_seq            // per client, not persisted daemon-side
```

Derived **attention**, in priority order, computed by the reducer and carried in the thread
summary so the tab, the header and the context-bar counts agree:

| Attention | When | Tab | Header | Context bar |
| --- | --- | --- | --- | --- |
| `NeedsYou(Permission)` | an open permission gate | amber dot | `needs you` | `needs you` |
| `NeedsYou(Question)` | an open question gate | amber dot | `needs you` | `needs you` |
| `NeedsYou(Plan)` | a settled plan waiting for approval | amber dot | `needs you` | `needs you` |
| `Working` | `session == Running` or `turn == Running` or background tasks alive | gray spinner | `working` | `working` |
| `Failed` | `turn == Failed` or `session == Error` or unexpected exit | `exited 1` red | `failed` | `failed` |
| `NeedsYou(Finished)` | turn completed and its `seq` is newer than `last_seen_seq` | amber dot | `needs you` | `needs you` |
| `Unread` | new non-terminal output since `last_seen_seq` | gray dot | — | — |
| `Idle` | otherwise | plain | `idle` | — |

The row order **is** the priority order, and it is rule 7 below: gates, then work, then failure,
then fresh completion. A turn that finished and has not been read yet may not shadow a session
that is running now or one that has died — a killed provider reads `exited 137` in red, not
`needs you`.

"Finished" is amber because the Signals board maps *agent finished* to *needs you*; it clears
when the user views the tab (the app reports `last_seen_seq`). A thread the user has never
opened is not unread. Broader worktree states win over agent states: offline > job phase >
agent state > session state.

The PTY fallback reuses only the `AttentionKind` vocabulary and the amber `NeedsYou` tab mark;
it does not imitate this reducer. Terminal silence remains a best-effort working/idle glyph and
never completes a turn or sends a notification. Only an explicit `fleet agent-status
permission|question|plan|finished` hook creates PTY attention, and repeated snapshots/events with
the same session attention do not re-notify. Native completion authority and seen-cursor behavior
remain unchanged.

Transition rules, all copied from t3code because each one closes a real race:

1. `Running` is set only by `TurnStarted` from the adapter, which emits it at submission, not
   at the first token. Deltas and items may only land in a known turn and item; a terminal event
   whose `turn` is not the active turn is dropped and logged.
2. Only the provider's authoritative primitive completes a turn (§4). Never silence, never a
   stream closing, never process liveness.
3. Before `TurnCompleted` is applied, every open item in that turn is closed (`ItemCompleted`
   with its last known status) so the transcript never shows a spinner on a finished turn.
4. Gates are independent of turns. A question or an actionable plan can remain open after a
   turn completed; a gate closes only on `GateResolved`.
5. Stream or process loss is `SessionExited { expected: false }` plus `RuntimeError`, which
   makes the turn `Failed`. Never inferred success. Recovery resumes by cursor with a new
   process; if that is impossible, the orphan is settled as failed explicitly.
6. A user message sent while running is **steering**: it joins the active turn (Claude) or the
   active prompt (OpenCode) and the turn settles only after the last in-flight send. A message
   queued while the agent works shows at 60 % opacity with `queued · [esc] unqueue` and is
   sent when the turn settles; settlement is withheld for a grace period after a queue.
7. Attention derives from gates, then work, then failure, then fresh completion. Seen state
   lives in the app.

## 4. Harness integration

Both adapters follow the same lesson from t3code: **speak the structured protocol, never
parse the terminal.** The detailed wire reference for the installed versions (Claude Code
2.1.263, OpenCode 1.17.18) is `research/harness-protocols.md`, with raw captures under
`research/fixtures/agents/`; this section fixes the decisions.

### 4.1 Claude Code

Transport: spawn the `claude` binary in the worktree with bidirectional stream-json over
stdin/stdout, the same protocol the official Agent SDK speaks to the CLI. No Node sidecar: the
CLI is a native binary and the protocol is newline-delimited JSON, which `serde_json` handles
directly. Fixtures recorded from the installed CLI version pin the shapes; a CLI version check
at session start gates unsupported versions to the terminal fallback.

- Launch: `claude -p --output-format stream-json --input-format stream-json --verbose
  --include-partial-messages --permission-prompt-tool stdio`, plus `--permission-mode`,
  `--model`, `--session-id=<uuid>` (fresh, Fleet-generated) or `--resume=<id>` and optionally
  `--fork-session`. `-p` is required for the standalone binary. Stdin stays open for the life
  of the session; stdout EOF is process lifetime, never a turn boundary. Environment inherits
  the user's login shell plus `FLEET_SESSION`/`FLEET_TERMINAL` so existing hooks keep working.
  The `initialize` control request is optional; Fleet sends it only to set a title.
- Turn: write one `{"type":"user","message":{...},"uuid":...}` line; the adapter emits
  `TurnStarted` at that write. A second `user` line while running is steering and is
  coalesced by the CLI (`user_message_uuids` on the reply, `queued_turn_count` on the result).
  Interrupt is `control_request { subtype: "interrupt", cancel_queued }`; **the interrupted
  turn still emits a `result`** (terminal reason `aborted_streaming` or `aborted_tools`), which
  is what settles it, not the control response.
- **Completion authority is the `result` message, exactly one per turn.** `subtype`,
  `is_error` (a `success` subtype can still carry `is_error: true`), `terminal_reason`, `usage`,
  `total_cost_usd`, `duration_ms` and `permission_denials` fill `TurnCompleted`. `modelUsage`
  and cost are cumulative for the process: take the latest, never sum. A `result` with no active
  turn only updates usage. Stdout closing without a `result` is a failed turn.
- Rendering source: `stream_event` deltas (`text_delta`, `thinking_delta`, buffered
  `input_json_delta`) move the cursor; `assistant` frames give the completed blocks (one block
  per frame, several frames share a `message.id`); `user` frames carrying `tool_result` close
  tool items and carry the structured `tool_use_result`; `system` subtypes map to
  `SessionStarted` (`init`: tools, slash commands, skills, model, permission mode),
  `Checkpoint` (`compact_boundary`), `Retrying` (`api_retry`), `Notice` (`informational`,
  `notification`), subagent rows (`task_started`/`task_progress`/`task_notification`), and
  `background_tasks_changed` (replace semantics). `session_state_changed` is a whole-session
  hint (`idle` / `running` / `requires_action`), not a turn terminal.
- **Permissions arrive as `control_request { subtype: "can_use_tool" }`** with `tool_name`,
  `input`, `permission_suggestions`, `decision_reason`, `title`, `description`, `tool_use_id`,
  and are answered with a same-id `control_response`. Allow is `{ behavior: "allow",
  updatedInput, updatedPermissions? }`; `allow for this session` echoes the suggested
  `addRules` with `destination: "session"`. Deny is `{ behavior: "deny", message, interrupt }`;
  `deny and stop` sets `interrupt: true`. Honour `default_to_no`, `suppress_always_allow_rule`
  and `requires_user_interaction`; sanitise ANSI in `decision_reason`.
  A `control_cancel_request` for a pending `can_use_tool` closes its gate (`GateResolved`,
  `ProviderClosed`): the CLI has stopped waiting, and there is no response to send.
  `AskUserQuestion` is a `can_use_tool` whose `input.questions` (1–4, each 2–4 options,
  `multiSelect`) becomes the Question gate; the answer is an allow whose `updatedInput` repeats
  the questions and adds `answers` keyed by the exact question text (arrays for multi-select,
  free text for "Something else…"). `ExitPlanMode` is a `can_use_tool` carrying `input.plan`;
  `approve and build` allows it unchanged, `ask for changes` denies it with the user's note so
  the model receives the feedback and stays in plan mode.
- Resume: the `session_id` from `init` is the cursor; a new process with `--resume=<id>`
  continues it, `--fork-session` branches it. Cost counters restart per process.

### 4.2 OpenCode

Transport: one managed `opencode serve --hostname 127.0.0.1 --port <free>` per thread, spawned
in the worktree with its own process group, readiness checked over HTTP within 30 s, stopped
with `SIGTERM` to the group then `SIGKILL`. Per thread rather than shared, because MCP and
directory registration are server-wide while the working directory belongs to the thread.
An external-server mode (user supplies a URL) is a later option.

- Every request carries `?directory=<canonical worktree path>`; the server can host several
  directories, so the routing key is `(base_url, directory, session_id)`. If a password is
  configured, HTTP Basic with user `opencode`.
- Session: `POST /session` creates (`{ title?, agent?, model: { id, providerID } }`);
  `GET /session/{id}` resumes. **Only a 404 permits creating a new session on resume**; any
  other error surfaces as a failure so context is never silently lost. Same canonical cwd
  resumes, changed cwd uses `POST /session/{id}/fork`.
- Turn: `POST /session/{id}/prompt_async` with `{ parts: [{ type: "text", text }], agent?,
  model?: { providerID, modelID } }` returns `204`, which is admission only; the adapter emits
  `TurnStarted` on admission with a 10 s budget and treats `session.status busy` as the
  execution-start level. Model and agent are chosen per prompt, never by patching shared
  config. Abort is `POST /session/{id}/abort`, acknowledged with a boolean and settled by the
  later idle; the assistant message then carries `error.name = MessageAbortedError`.
- Events: `GET /event?directory=…` is SSE with one JSON object per `data:` line, shape
  `{ id, type, properties }`, no replay cursor. `message.part.delta { partID, field, delta }`
  is an append; `message.part.updated { part }` is a **cumulative replacement by `part.id`**,
  never appended. `ToolPart` arrives repeatedly with the same id as `pending → running →
  completed | error`: upsert, do not create four rows. `ReasoningPart` is the thinking line,
  `PatchPart` the inline diff, `CompactionPart` and `RetryPart` checkpoints and retries,
  `todo.updated` the Todo row, `step-finish` is one model step and never a turn end.
- **Completion authority is `session.status { status: { type: "idle" } }`** for that session,
  or a valid `GET /session/status` map from which the session is absent. `busy` and `retry`
  mean running. SSE silence is never idle: on a quiet or dropped stream the adapter polls
  `/session/status` with a 1 s request timeout and 250 ms → 5 s backoff, then reconciles
  messages, `GET /permission` and `GET /question` as recovery truth. `session.error` does not
  imply idle. Exit of the supervised `serve` child is `SessionExited { expected: false }`;
  SSE EOF alone is subscription loss, confirmed by a health request.
- Gates: `permission.asked { id, permission, patterns, always, tool }` → Permission gate
  answered with `POST /permission/{id}/reply { reply: once | always | reject }`;
  `question.asked { id, questions }` → Question gate answered with `POST /question/{id}/reply
  { answers: string[][] }` (one array per question) or `/reject`. **`always` is stored per
  directory** in this version and can widen later sessions in the worktree, so Fleet copy says
  `allow for this directory` and defaults the focused key to `once`. In full-access mode the
  adapter answers `once` automatically rather than persisting a broad grant.
- Plan: there is no plan-proposed event. Fleet knows it selected the `plan` agent, so the
  completed assistant text at idle is the proposal and becomes the Plan gate; approving switches
  the next prompt to the `build` agent.
- Modes and copy: agents are `build agent` / `plan agent` from `GET /agent`; the metadata row
  reads `build agent · claude-sonnet-5 · high · asks before edits`. Title comes from
  `Session.title` via `session.updated`.

### 4.3 Authoritative signals

| Signal | Claude Code | OpenCode |
| --- | --- | --- |
| Session ready | `system/init` | `GET /session/{id}` ok and SSE `server.connected` |
| Turn started | adapter, on writing the `user` line | `prompt_async` 204 (admission), `session.status busy` (execution) |
| Streaming text | `stream_event` `content_block_delta` | `message.part.delta` (append); `message.part.updated` replaces |
| Tool started / done | `assistant` `tool_use` block / `user` `tool_result` | `ToolPart` `pending`/`running` → `completed`/`error`, upserted by id |
| Permission | `control_request can_use_tool` | `permission.asked`; missed ones from `GET /permission` |
| Question | `can_use_tool` for `AskUserQuestion` | `question.asked`; missed ones from `GET /question` |
| Plan | `can_use_tool` for `ExitPlanMode` with `input.plan` | plan agent selected + assistant text at idle |
| Turn completed | exactly one `result` (`subtype`, `is_error`, `terminal_reason`) | `session.status idle`, or absent from `/session/status`; never `step-finish` |
| Turn aborted | `interrupt` control, then that turn's `result` with an aborted terminal reason | `POST …/abort`, then idle; `MessageAbortedError` on the assistant message |
| Error | error `result` or `is_error: true`; `api_retry` is not terminal | `session.error`, `AssistantMessage.error`; `retry` status is not terminal |
| Process death | child exit or stdout EOF before the expected `result` | supervised `serve` child exit; SSE EOF is only subscription loss |
| Resume cursor | `session_id` → `--resume=<id>` | `(base_url, directory, session_id)` → `GET /session/{id}`, resubscribe, reconcile |

Implementation invariants that apply to both: one writer per stdin or HTTP client with a single
request-id registry; duplicate settlements are idempotent; unknown message types and fields are
kept as diagnostics and never break the stream; provider ids (session, turn, message, part,
tool call) are persisted independently of the live transport.

## 5. Rendering model

The app holds one `Entity<AgentThreadView>` per open agent tab. It owns a keyed row model
built from the thread projection: `Vec<Row>` where a row is one of `UserBlock`, `AssistantText`,
`Thinking`, `ToolRow { children }`, `WorkedFold`, `TurnFooter`, `DecisionCard`, `ErrorCard`,
`CheckpointLine`, `Notice`, `QueuedMessage`, `EmptyState`. A `Notice` is §3.2's user-facing
provider signal — a config warning, a deprecation, `Stop hook error occurred` — drawn as one
muted line with an amber glyph; an unrecognised provider frame is a tracing diagnostic and never
a notice, so the transcript keeps the signals and the log keeps the noise.

A notice is deliberately **not** foldable and carries no `[⏎] show`. Neither wire gives a notice
a body: Claude's harness messages arrive as a single already-complete sentence and its stderr is
the CLI's own, never addressed to the thread, and OpenCode publishes notices as plain strings
too. DESIGN-SYSTEM §4 does not list a command that cannot fire, so the row is a plain line —
adding the keycap would promise a detail view with nothing behind it. If a provider ever sends a
notice body, the row becomes a `ToolRow`-style fold and the hint returns with it.

Update policy, borrowed from t3code's streaming fast path: a `ContentDelta` mutates only the
text of the row for that `item` and notifies; structural events (`ItemStarted`,
`ItemCompleted`, `GateOpened`, `TurnCompleted`) rebuild grouping (folds, footers). Deltas are
coalesced in the daemon (one event per item per ~16 ms) and again in the bridge so a fast
model does not schedule a render per token. Turn metadata is withheld until the turn completes,
so the footer never moves under the reader.

Tool rows keep their 30 px column geometry while streaming so the list does not jitter.
Summary text per kind follows the canvas table (Read → path · `120 lines`; Bash → command ·
`exit 0 · 1.2s`; Edit → path `+14 −3` · `[⏎] diff`; Agent → `explore · find every caller` ·
`8 tools · 24s`). A failed tool stays exposed when the turn folds; only successful rows fold.
Expanded bodies are bounded in height with their own scroll.

Diffs render inline under Edit/Write rows using the ADR 0005 stack (`syntect`, `similar`,
uniform rows) through `fleet_lazygit::diff_view::DiffView`, which takes unified-diff text so the
kit gains no `fleet-git` dependency.

`[u] revert this edit` and `[u] revert turn` are **not built yet**. The design is Fleet-owned
checkpoints — the daemon records a hidden git ref of the worktree before each turn and per file
before each edit, and revert restores files from it, deliberately decoupled from provider
completion and from provider conversation rewind. Until that lands, `u` says so instead of
silently doing nothing or running a different, destructive action (§10).

## 6. Persistence and reconnect

- **Transcript:** one append-only NDJSON log per thread at
  `$FLEET_HOME/agents/<thread_id>/events.ndjson`, each line a `SeqEvent`. Written by the reducer
  before the event is broadcast. Cumulative bodies are not rewritten on every update. Compaction
  on close (a `snapshot.json` plus an `events.ndjson` tail, so an old thread does not cost hours
  of deltas) is designed but not built: the daemon replays every thread's whole log at start
  (§10).
- **Index:** thread metadata (id, worktree, provider, title, created, last activity, resume
  cursor, model, mode, last outcome) in a small `agents/index.json` handled with the same
  discipline as `StateStore` (serialised mutation, atomic write, quarantine on corruption). It
  is not part of `PersistedState` version 1; it gets its own file and version.
- **Client reconnect:** `Snapshot` carries thread summaries; opening a tab requests
  `AgentThreadSnapshot { thread, snapshot, from_seq }` and then subscribes to
  `AgentEvent { thread, seq, event }`. A gap in `seq` triggers a resync from the last applied
  `seq`. This mirrors the terminal frame recovery rule rather than inventing a new one.
- **Daemon restart:** the manager loads the index and replays each thread's log. A thread the
  log leaves `Starting` or `Running` is an orphan and is settled explicitly, as appended events
  rather than as a silent state edit: with a resume cursor it gets `TurnAborted { ProviderExited }`
  (if a turn was running) then `SessionStateChanged(Stopped)`, and is resumed lazily with a new
  adapter the next time it is opened; without one it gets `SessionExited { code: None, expected:
  false }`. It never tries to reattach a PID. Threads stay browsable read-only regardless.

## 7. Protocol additions (`fleet-proto`)

`PROTOCOL_VERSION` is **6**. The `RequestBody` variants beside the session requests are
`AgentThreadList`, `AgentThreadCreate { worktree, provider, model, mode, resume_cursor, title }`,
`AgentThreadOpen { thread, from_seq }`, `AgentThreadClose`, `AgentSend { thread, input }`,
`AgentInterrupt`, `AgentRespond { thread, gate, answer }`, `AgentSetMode`, `AgentSetModel`,
`AgentMarkSeen { thread, seq }` and `AgentStop`. `AgentThreadCreate` carries the *published*
`WorktreeId`; the daemon resolves the trusted canonical path itself. `ResponseBody` answers with
`AgentThreads`, `AgentThreadCreated`, `AgentThreadSnapshot { projection, events_after }` and
`AgentAck`; `Event::Agent { thread, event: SeqEvent }` and
`Event::AgentSummary(AgentThreadSummary)` carry the stream and the tab/context-bar state.
`Snapshot::agent_threads: Vec<AgentThreadSummary>` is `#[serde(default)]`, so a version-4
snapshot payload still decodes.

`AgentRevert { thread, checkpoint }` is **not** in the protocol: the checkpoint service it would
address does not exist yet (§5, §10).

The CLI drives the same requests without any UI:
`fleet agent list|new|send|respond|interrupt|stop|tail`, with `fleet agent terminal` for the PTY
fallback. `fleet agent tail <thread> [--replay]` prints one JSON `SeqEvent` per line.

## 8. UI kit additions

| Component | Contract |
| --- | --- |
| `TranscriptList` | Variable-height rows over GPUI `list` with bottom alignment, keyed rows, overdraw, jump-to-latest, `^s [` scroll mode. Not `uniform_list`; ADR 0005's uniform-row rule applies to diffs, which stay uniform inside their row. |
| `MultilineInput` | Multi-line `TextInput` sibling: wrapping, IME, paste, history (`↑`), Enter/Shift+Enter, `@` and `/` triggers. |
| `Markdown` | Paragraphs, headings, inline code, fenced code, lists, quotes, rules, emphasis, links. Parses a *prefix* safely: every decided block stays a block, at the same index, as the stream grows. No tables or images in v1, and fenced blocks are coloured locally rather than through the ADR 0005 `syntect` stack. |
| `ToolRow` | The 30 px row with state glyph, kind, summary, result, optional nested children region (16 px indent, 1 px left divider). |
| `DecisionCard` | Permission, question, plan variants; amber left bar; keycap actions; owns key routing while open. |
| `DiffView` | Lives in `fleet_lazygit::diff_view`, not the kit, because it takes unified-diff text and keeps the ADR 0005 row stack; inline mode uses the `diff_added` / `diff_removed` tokens (14 % tints). |
| `KeyHint` | Reused for every visible shortcut. |

Their exact contracts — constructors, events, states and usage rules — are `DESIGN-SYSTEM.md`
§6.6; the six fixed dimensions are `components::agent::metrics` and are listed in §2.8 there.
`gallery_agent` is the example binary that shows every state.

All colours come from tokens. The canvas palette maps onto the existing semantic tokens
(`bg.app #0E1013`, `bg.panel #16181D`, `bg.elevated #1B1E24`, `border.subtle #22262E`,
`text.secondary #8A9099`, `accent.focus #58A6FF`, success `#3FB950`, warning `#D29922`,
error `#F85149`); missing ones are added to `DESIGN-SYSTEM.md`, not styled locally.

## 9. Keymap

The key context is `Agent` with sub-states `AgentIdle`, `AgentWorking` and `AgentDecision`,
which routes to `AgentPermission` / `AgentQuestion` / `AgentPlan` so three cards cannot collide
on `y`; a focused transcript row is `AgentRow`. `^s` stays the only global prefix.
`KEYMAP.md` § *Native agent thread* is authoritative.

| Context | Keys |
| --- | --- |
| Idle | `⏎` send · `⇧⏎` newline · `⇧⇥` plan mode · `/` commands · `@` files · `↑` history · `^s m` model · `^s [` scroll (toggles; `esc` also leaves) · `^s a`/`^s A` new thread · `^s x` close tab · `^s F` terminal fallback |
| Working | `esc` stop · `⏎` queue message · `^s [` scroll · `^s a`/`^s A` new thread · `^s x` close tab · `^s F` terminal fallback |
| Decision: permission | `y` allow once · `a` allow for this session · `n` deny · `e` edit the command (only where the provider accepts one) · `esc` deny and stop |
| Decision: question | `1–4` choose · `space` toggle (multi-select) · `⏎` answer |
| Decision: plan | `y` approve and build · `n` ask for changes · `⏎` view full plan |
| Row focus | `⏎` expand/collapse · `u` revert edit or turn · `o` open in editor |

Collisions are resolved in `keymap.rs` as ADR 0004 prescribes; the terminal fallback keeps the
`Agent > Terminal` context and the `TERMINAL` word. Two caveats are recorded in `KEYMAP.md`:
`Agent > AgentRow` is bound but nothing focuses a row yet, and the rest of the Workspace prefix
table (`^s s`, `^s 1`–`9`, `^s h`/`l`, …) is not bound inside an agent tab.

## 10. Status

Each phase was shippable on its own and kept the terminal path working. This is where they
landed; every phase kept the terminal fallback working, and nothing below is a known-broken
surface — a follow-up is a surface that is not there yet and says so where the user meets it.

| # | Phase | Status |
| --- | --- | --- |
| 1 | **Domain, protocol, fixtures.** `fleet-core::agents` types and the reducer, `fleet-proto` variants at version 6, harness captures under `research/fixtures/agents/`, replay tests asserting thread state and attention for a plain turn, a tool turn, a permission, a question, a plan, an interrupt and process death | **done** |
| 2 | **Daemon adapters and store.** Claude over stream-json, OpenCode over HTTP + SSE, `AgentSessionManager`, per-thread append-only log, versioned index, 16 ms delta coalescing, restart recovery, `fleet agent` CLI | **done** |
| 3 | **Read-only transcript.** `AgentThreadView` with user blocks, assistant Markdown, thinking, tool rows, folds, footers, checkpoints and error cards; the tab badge, the session-header word, the context-bar counters and `NeedsYou` notifications | **done** |
| 4 | **Composer and decisions.** `MultilineInput`, send, queue, steer, interrupt, the three decision cards with key routing, the `/` `@` and model pickers, mode switching, empty state | **done** |
| 5 | **Diffs and revert.** `DiffView` extracted and rendering inline under edit rows | **partial** — the diff landed; Fleet-owned checkpoints, `AgentRevert`, `[u] revert this edit` / `[u] revert turn` and the turn-level diff surface did not (§5, §7) |
| 6 | **Polish and migration.** Tab titles from the first message or `Session.title`, `^s a`/`^s A` defaulting to native with `^s F` as the explicit fallback, and the docs above | **done** |

Known follow-ups, each already named where it is visible:

| Follow-up | Where it shows today |
| --- | --- |
| Turn checkpoints and revert | `u` answers `revert needs turn checkpoints, which are not built yet` instead of doing nothing (§5, `KEYMAP.md`) |
| Transcript row focus | `Agent > AgentRow` is bound and handled but never entered, so `⏎` / `u` / `o` on a row cannot fire; a **click** on a tool row, a thinking line or a fold expands it in the meantime (`KEYMAP.md`, `APP-CONTRACTS.md` §3) |
| The Workspace prefix inside an agent tab | only `^s m [ a A x F` are bound there; `^s s`, `^s 1`–`9` and `^s h`/`l` are not (`KEYMAP.md`) |
| Transcript compaction on close | the daemon replays a thread's whole log at start (§6) |
| OpenCode attachments | a send carrying attachments fails with a typed `Protocol` error naming the gap, rather than dropping them (§4.2) |
| GUI smoke procedure for the agent tab | ADR 0007's driver script has no agent-tab pass yet |

## 11. Risks and open questions

- **Protocol drift.** The stream-json control protocol is what the SDK uses but is not
  documented as a public contract. Mitigation: a version gate, fixtures per CLI version, the
  terminal fallback, and `SeqEvent.raw`, which retains the provider's own event type name for
  every stored event. Surfacing that raw record behind `[⏎] raw` in the transcript is a
  follow-up; today it is read from the log.
- **Claude interrupt scope.** Interrupting a Claude turn may also stop background subagent
  work; t3code chose to stop the whole session. Fleet sends the interrupt and treats the turn as
  `Interrupted`, surfacing background loss in the footer.
- **Cost of one OpenCode server per thread.** Acceptable for a handful of tabs; revisit with an
  external-server mode if users run many.
- **Markdown scope.** A focused in-house renderer was chosen over Zed's `markdown` crate to
  respect ADR 0003 and keep the dependency graph small (ADR 0010); tables and images come later.
- **Cancelled state.** The canvas defines no cancelled block; Fleet renders an interrupted turn
  with a footer `stopped · 12s · 3.1k tokens` and no error card.
- **Human PR review** is not an agent state; it stays in Hub/Pull Requests.
