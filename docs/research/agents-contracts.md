# Native agents contract reference

This document is the compact public-API and serialized-shape reference for native agents and
delegations: names, signatures, wire shapes, and module paths. The `fleet-core` and `fleet-proto`
shapes below are current and are pinned byte-for-byte by
`crates/fleet-proto/tests/agent_compatibility.rs`; a disagreement with those goldens is a bug.
All domain and wire enums use Serde `snake_case` variant names, and struct fields use `camelCase`,
unless a declaration below says otherwise.

> **History note.** The initiative began as a Claude Code + OpenCode freeze. The shipped rewrite
> replaced OpenCode with Codex and replaced the original optional-field transcript model with the
> typed shapes recorded below. The implementation-inventory sections retain a few explicitly
> marked pre-rewrite entries for archaeology; they are not wire contracts.
>
> Phase 5a replaced the `fleet-ui-kit` half. `components/agent/decision_card.rs` is gone:
> approvals and questions live in a `DecisionDock` docked to the composer (`decision.rs` +
> `decision_dock.rs`), a proposed plan is a transcript row with no buttons, and `AllowDirectory`,
> `ApprovePlan`, `AskForChanges` and `ViewPlan` were retired for `Implement` / `Refine` /
> `Previous`. `TranscriptRow` is a struct — `{ id: TranscriptRowId, kind: TranscriptRowKind,
> attached }` — with the eighteen kinds of §5, `ToolRowState` gained `Stopped` and `Severe` and
> lost `Error`, and `TranscriptList` gained the three-state scroll machine, `set_thread`,
> `set_row_body` and the row-focus verbs. `docs/DESIGN-SYSTEM.md` §6.6 is the current inventory.

`docs/NATIVE-AGENTS.md` is the specification, `docs/decisions/0010-native-agents.md` records why
the load-bearing choices are what they are, and `docs/research/harness-protocols.md` is the wire
reference for the two harnesses.

## `fleet-core`

Everything below is publicly re-exported from `fleet_core::agents` by
`crates/fleet-core/src/agents/mod.rs`. The former popup-process recognition API remains
`RECOGNIZED_AGENTS: [&str; 3]` and `recognized_agent(&str) -> Option<&'static str>` in
`agents/recognition.rs`.

### Identity — `crates/fleet-core/src/agents/ids.rs`

`ThreadId`, `DelegationId`, `TurnId`, `ItemId`, and `GateId` are transparent UUID newtypes. Each
implements
`new() -> Self`, `from_uuid(Uuid) -> Self`, `as_uuid(self) -> Uuid`, `Default`, `Display`,
`FromStr<Err = uuid::Error>`, and conversions to/from `Uuid`. `Seq(pub u64)` is a transparent,
ordered per-thread cursor with `next(self) -> Seq` (saturating) and `Display`.

### Delegations — `crates/fleet-core/src/agents/delegation.rs`

`Delegation` is a camel-case serialized struct with this public shape:

```rust
Delegation {
    id: DelegationId,
    caller: ThreadId,
    caller_turn: TurnId,
    caller_item: ItemId,
    child: ThreadId,
    provider: AgentKind,
    depth: u8,
    brief: String,
    expectation: String,
    eager: bool,
    status: DelegationStatus,
    status_payload: Option<String>,
    result: Option<DelegationResult>,
    nudges: u8,
    recoveries: u8,
    delivery: DeliveryState,
    created: DateTime<Utc>,
    finished: Option<DateTime<Utc>>,
    headline: Option<String>,
}
```

The JSON keys are `callerTurn`, `callerItem`, `statusPayload` and so on. `eager: false`, `None`
optional fields and an empty result file list are omitted; `nudges` and `recoveries` default to
zero when absent. The bearer token is intentionally not on this type: the daemon persists its
SHA-256, while only `RequestBody::DelegationComplete` carries the plaintext token.

- `DelegationStatus = Starting | Running | Blocked | Settling | Succeeded | Incomplete | Failed |
  Cancelled` serializes as snake-case strings. `is_terminal` is true for the final four,
  `is_live` is its inverse, and `word` returns `starting | working | blocked | settling | done |
  incomplete | failed | cancelled`.
- `DelegationResult { text: String, files_changed: Vec<String>, source: ResultSource,
  elided: bool }` is camel-case. `ResultSource = Reported | LastAssistantText` serializes as
  `reported | last_assistant_text`.
- `DeliveryState = Pending | Delivered { seq: Seq, turn: TurnId } | Undeliverable {
  reason: String }` uses the tagged shape `{ "type": snake_case, "data": ... }`.
  `is_pending` and `word() -> "pending" | "delivered" | "undeliverable"` are the compact
  projection helpers.
- `Delegation::elapsed(now)` measures `created` to `finished`, or to `now` while live.

### State — `crates/fleet-core/src/agents/state.rs`

- `AgentKind = Claude | Codex`; `executable(self) -> &'static str` gives `claude` or `codex`, and
  `display_name(self) -> &'static str` gives `Claude` or `Codex`.
- `WaitingReason = UsageLimit { window: String, resets_at: DateTime<Utc> }` is an adjacently
  tagged reason. `SessionState = Starting | Ready | Running | Waiting(WaitingReason) | Stopped |
  Error`; the default is `Ready`.
- `StopCause = User | ProviderExit` serializes as `user | provider_exit`. It is kept on the
  durable thread record so an intentional Stop is distinguishable from a provider crash.
- `TurnState = None | Running(TurnId) | Settled(TurnId, TurnOutcome)`; the default is `None`.
- `AttentionKind = Permission | Question | Plan | Finished`.
- `Attention = NeedsYou(AttentionKind) | Working | Waiting | Failed | Unread | Idle` implements
  total ordering. `rank(self) -> u8` is exactly: permission 8, question 7, plan 6, working 5,
  waiting 4, failed 3, finished 2, unread 1, idle 0.
- `PermissionMode = Ask | AcceptEdits | Plan | Auto | DontAsk | FullAccess`; the default is
  `Ask`. Stable serde names are `ask`, `accept_edits`, `plan`, `auto`, `dont_ask`, and
  `full_access`; the two Claude-native additions are part of the protocol-8 compatibility
  boundary because older peers cannot decode unknown closed-enum values.
- `ModelSelection { model: String, effort: Option<String>, provider: Option<String> }` carries
  provider-native model coordinates.
- `ReasoningEffortDescriptor { id, description }` and `ModelDescriptor { id, display_name,
  efforts, default_effort }` preserve the harness's model catalogue and copy.
- `ControlCost = InPlace | RestartWithResume | NotSupported`; `ResumeSupport = None | ByCursor {
  fork }`; `SteerSupport = None | Implicit | Explicit { compare_and_swap }`; and
  `InterruptSupport = HardClose | Receipted { cancel_queued }` describe control truthfully rather
  than flattening it to booleans.
- `SandboxAxes = One | Three` and `ReasoningChannels = One | Two` name the two harness shapes.
  `HarnessCapabilities { version: semver::Version, resume, steer, interrupt, history_readback,
  native_compaction, turn_diff, attention_flags, async_questions, secret_answers, sandbox_axes,
  reasoning_channels, live_context_meter, model_switch, effort_switch, mode_switch,
  modes: Vec<PermissionMode>, declared: BTreeSet<String> }` is camel-case and defaulted as a
  whole. `modes` is in picker order: Claude publishes `[Ask, AcceptEdits, Plan, Auto, DontAsk,
  FullAccess]`; Codex publishes `[Ask, AcceptEdits, Plan, FullAccess]`; the vector defaults empty
  for older payloads.
- `AccountStatus = SignedOut | SignedIn(AccountInfo)` and `AccountInfo { kind: AccountKind }` are
  adjacently tagged/camel-case respectively. `AccountKind = ChatGpt { email, plan } | ApiKey |
  Other(String)` is adjacently tagged; the `ChatGpt` wire tag is exactly `chatgpt`.

### Provider input — `crates/fleet-core/src/agents/provider.rs`

- `AttachmentSource = Path(PathBuf) | Url(String) | Base64(String)` identifies attachment data.
- `Attachment { name: Option<String>, media_type: String, source: AttachmentSource }` is one
  ordered user attachment.
- `StartRequest { thread: ThreadId, worktree_path: PathBuf, provider: AgentKind,
  model: Option<ModelSelection>, mode: PermissionMode, resume_cursor: Option<String>,
  fork: bool, env: BTreeMap<String, String>, sandbox: SandboxPolicy, approval_policy:
  ApprovalPolicy, permission_profile: Option<String>, title: Option<String> }` fully describes a
  provider launch or resume. `SandboxPolicy = ReadOnly | WorkspaceWrite | DangerFullAccess` and
  `ApprovalPolicy = Untrusted | OnRequest | Never | Granular(BTreeMap<String, bool>)` use
  kebab-case variant tags.
- `UserInput { text: String, attachments: Vec<Attachment>, item: Option<ItemId>, origin:
  MessageOrigin }` is a submitted or steered prompt. `item` is omitted when absent; the default
  `origin: User` is omitted for byte compatibility.

### Items — `crates/fleet-core/src/agents/items.rs`

- `ToolKind = Read | Edit | Write | Bash | Search | Grep | Fetch | Agent | Todo | Skill |
  Mcp { server: String } | Unknown { name: String }` is the normalized tool taxonomy.
- `ToolCall { kind, name, input, summary, result, output, diff, exit_code, duration_ms, extra }`
  keeps structured tool data. `ToolPatch` carries an `Option` replacement for each of those
  fields.
- `ItemKind = UserMessage { text, attachments, steered, origin } | AssistantText { text } |
  Reasoning { summary: BTreeMap<u32, String>, raw: BTreeMap<u32, String> } | Tool(Box<ToolCall>) |
  Subagent { name, description, result } | Plan { text } | Error { message } | Delegation { id,
  provider, child, status }` drives transcript projection.
- `MessageOrigin = User | Delegation { id: DelegationId }` uses the tagged
  `{ "type": snake_case, "data": ... }` shape. `User` is the default and is omitted from a
  `UserMessage`; a delegated result keeps its delegation id on the message.
- `ItemPayloadPatch` mirrors the replaceable fields of every `ItemKind`: `UserMessage { text,
  attachments, steered }`, `AssistantText { text }`, `Reasoning { summary, raw }`,
  `Tool(Box<ToolPatch>)`, `Subagent { name, description, result }`, `Plan { text }`, `Error {
  message }`, and `Delegation { status }`. `ItemPatch { payload: Option<ItemPayloadPatch>, status:
  Option<ItemStatus> }` wraps those variant-specific replacements and lifecycle state.
- `ToolDiff { path: PathBuf, added: u64, removed: u64, unified: String }` carries a complete
  inline unified diff and counts.
- `Item { id: ItemId, turn: TurnId, parent: Option<ItemId>, kind: ItemKind, status: ItemStatus,
  children: Vec<ItemId>, started: DateTime<Utc>, ended: Option<DateTime<Utc>> }` is the current
  ordered transcript item; payload fields live only inside `kind`.

### Gates — `crates/fleet-core/src/agents/gates.rs`

- `ProviderOptionId(pub String)` is the opaque provider reply/suggestion identifier.
- `PermissionChoice = AllowOnce | AllowSession | AllowDirectory | Deny | Edit | DenyAndStop`.
- `PermissionOption { id: ProviderOptionId, label: PermissionChoice }` preserves exact provider
  option mapping while presenting normalized semantics.
- `QuestionOption { id: ProviderOptionId, label: String, description: String }` is one provider
  question choice.
- `Question { id: String, header: String, prompt: String, options: Vec<QuestionOption>,
  multi_select: bool, allows_other: bool, is_secret: bool, blocking: bool }` preserves both
  harnesses' question shapes. `blocking` defaults true; the other booleans default false.
- `GateKind = Permission { item: Option<ItemId>, tool: ToolKind, title: String, payload: String,
  rationale: Option<String>, options: Vec<PermissionOption> } |
  Question { questions: Vec<Question> } | Plan { markdown: String, steps: Vec<String> }`.
- `PlanAnswer = Approve | AskForChanges { note: String }`.
- `GateAnswer = Permission { choice: PermissionChoice, edited_payload: Option<String> } |
  Question { answers: Vec<Vec<String>> } | Plan(PlanAnswer)`.
- `GateResolver = User | Auto | Timeout | ProviderClosed` names the authoritative closer.
- `OpenGate { id: GateId, turn: Option<TurnId>, kind: GateKind, opened_seq: Seq, blocked_since:
  Option<DateTime<Utc>> }` is one actionable projected gate. The last field was added so turn
  footers can subtract time parked on overlapping user gates.

### Events — `crates/fleet-core/src/agents/event.rs`

- `SessionInfo { provider: AgentKind, resume_cursor: Option<String>, model:
  Option<ModelSelection>, mode: PermissionMode, tools: Vec<String>, commands: Vec<String>, skills:
  Vec<String> }` is the compact initialization metadata value.
- `Usage { input_tokens: u64, output_tokens: u64, reasoning_tokens: u64,
  cache_read_tokens: u64, cache_write_tokens: u64, total_tokens: u64,
  web_search_requests: u64, tool_uses: u64, extra: BTreeMap<String, serde_json::Value> }`
  normalizes known usage and retains additive provider fields via flattened `extra`.
- `FileDelta { path: PathBuf, added: u64, removed: u64 }` is a completed-turn file summary.
- `TurnOutcome = Completed | Error { message: Option<String> } | Interrupted | Denied |
  MaxTurns | BudgetExhausted | Other { reason: String }` preserves provider terminal reasons.
- `AbortReason = User | SessionStopped | ProviderExited | Timeout | Superseded | Other(String)`.
- `ItemPatch` has the typed payload/status shape described under Items rather than a flat set of
  optional strings.
- `StreamKind = AssistantText | ReasoningSummary { part: u32 } | ReasoningRaw { part: u32 } |
  CommandOutput | PlanText` selects an append-only content channel.
- `CheckpointKind = CompactBoundary { before: u64, after: Option<u64> } |
  Resumed { age_ms: u64 }` records transcript observability boundaries.
- `ItemStatus = InProgress | Completed | Failed | Denied | Stopped`; the default is `InProgress`.

`AgentEvent` is internally tagged as `{ "type": snake_case, "data": ... }` and has these exact
variants:

```rust
SessionConfigured { provider: AgentKind, resume_cursor: Option<String>,
    model: Option<ModelSelection>, models: Vec<ModelDescriptor>, mode: PermissionMode,
    tools: Vec<String>, commands: Vec<String>, skills: Vec<String> }
MetadataChanged { title: Option<String>, mode: Option<PermissionMode>,
    model: Option<ModelSelection>, skills: Option<Vec<String>> }
SessionStateChanged(SessionState)
SessionActivity { phase: String }
SessionExited { code: Option<i32>, expected: bool }
TurnStarted { turn: TurnId, user_item: ItemId }
TurnSettled { turn: TurnId, outcome: TurnOutcome, usage: Usage,
    duration_ms: u64, files_changed: Vec<FileDelta> }
TurnAborted { turn: TurnId, reason: AbortReason }
TurnDiff { turn: TurnId, unified: String, files_changed: Vec<FileDelta> }
PlanSteps { turn: TurnId, steps: Vec<String> }
ItemStarted { turn: TurnId, item: ItemId, kind: ItemKind, parent: Option<ItemId> }
ContentDelta { item: ItemId, stream: StreamKind, delta: String }
ItemUpdated { item: ItemId, patch: ItemPatch }
ItemCompleted { item: ItemId, status: ItemStatus }
GateOpened { gate: GateId, turn: Option<TurnId>, kind: GateKind }
GateResolved { gate: GateId, answer: GateAnswer, by: GateResolver }
GateWithdrawn { gate: GateId }
PlanProposed { gate: GateId, turn: TurnId, markdown: String, steps: Vec<String> }
TokenUsage { turn: TurnId, usage: Usage, context_pct: f32, cost_usd: Option<f64> }
RateLimits { limits: serde_json::Value }
Compacted(CheckpointKind)
Retrying { attempt: u32, retry_in_ms: u64, reason: String }
ModelRerouted { from: String, to: String, reason: String }
RuntimeError { fatal: bool, message: String }
AccountChanged { account: AccountStatus }
Notice(String)
Unknown { method: String }
```

`MetadataChanged` carries a post-start change to projected session metadata: the reducer applies
the fields that are `Some`, and each is omitted from the JSON when `None`. It exists because mode,
model, skills and `Session.title` are rendered state, and §3/§6 make the event log the only way a
client learns about a change to them.

`SeqEvent { seq: Seq, at: DateTime<Utc>, raw: Option<String>, event: AgentEvent }` is the durable,
time-stamped reducer input. `raw` retains only a provider event/type name for diagnostics.

### Projection — `crates/fleet-core/src/agents/projection/{mod.rs,summary.rs}`

- `TurnEnd { outcome: TurnOutcome, usage: Usage, duration_ms: u64,
  files_changed: Vec<FileDelta> }` stores terminal facts.
- `TurnRecord { id: TurnId, user_item: Option<ItemId>, started_at: DateTime<Utc>,
  ended: Option<TurnEnd>, blocked_ms: u64 }` stores one chronological user turn. `blocked_ms` is
  omitted at zero and is subtracted from the provider's wall-clock duration in the footer.
  `TurnRecord::footer(&self) -> Option<TurnFooter>` is `Some` only for a settled turn.
- `TurnFooter { duration_ms: u64, tokens: u64, files_changed: usize, added: u64, removed: u64 }`
  is the §5 turn-footer row's data, summed in the reducer rather than in a view, so the CLI and
  the tab quote the same numbers.
- `RetryState { attempt: u32, retry_in_ms: u64, reason: String }` stores current backoff.
- `CheckpointRecord { kind: CheckpointKind, seq: Seq, after_turn: Option<TurnId> }` is one
  recorded compaction or resume boundary, in observation order; `ThreadProjection::checkpoints`
  carries them so §5's `CheckpointLine` row can be built.
- `NoticeRecord { text: String, seq: Seq, after_turn: Option<TurnId> }` is one user-facing
  provider notice; `ThreadProjection::notices` carries them so §5's `Notice` row can be built.
- `ThreadProjection { thread: ThreadId, parent: Option<ThreadId>, worktree: WorktreeId,
  provider: AgentKind, title: String,
  session: SessionState, turn: TurnState, gates: Vec<OpenGate>, items: Vec<Item>,
  turns: Vec<TurnRecord>, background_tasks: Vec<ItemId>, checkpoints: Vec<CheckpointRecord>,
  notices: Vec<NoticeRecord>, last_seq: Seq, last_completed_seq: Option<Seq>,
  last_nonterminal_seq: Option<Seq>, last_activity: Option<DateTime<Utc>>,
  cumulative_usage: Usage, cumulative_cost_usd: Option<f64>, context_pct: f32,
  model: Option<ModelSelection>, models: Vec<ModelDescriptor>, skills: Vec<String>,
  mode: PermissionMode, account: Option<AccountStatus>, exit_code: Option<i32>,
  retrying: Option<RetryState> }` is materialized state. `last_completed_seq` is the sequence of
  the newest turn-settling event and `last_nonterminal_seq` the newest event that is *not* one, so
  `Finished` and `Unread` can be told apart from a cursor alone without re-walking the transcript.
- `ThreadProjection::new(ThreadId, WorktreeId, AgentKind) -> Self` creates an empty starting
  projection; `apply(&mut self, &SeqEvent) -> Result<(), ProjectionError>` is the reducer;
  `accepts(&self, &SeqEvent) -> Result<(), ProjectionError>` answers the same question without
  mutating, so the daemon can refuse an event before it is persisted; `attention(&self, Seq) ->
  Attention` derives the badge; and `summary(&self, Seq) -> AgentThreadSummary` creates compact
  client state.
- `AgentThreadSummary { thread: ThreadId, parent: Option<ThreadId>, worktree: WorktreeId,
  host: Option<HostId>, provider: AgentKind,
  title: String, attention: Attention, session: SessionState, turn: TurnState, last_seq: Seq,
  last_activity: Option<DateTime<Utc>>, last_completed_seq: Option<Seq>,
  last_nonterminal_seq: Option<Seq>, exit_code: Option<i32> }` backs snapshots and tabs.
  The broadcast `attention` is derived against an *unseen* thread, because one summary reaches
  every subscriber and folding one client's cursor into it would clear the amber dot on all the
  others; `attention_for(&self, last_seen: Seq) -> Attention` re-derives the two seen-relative
  attentions (`Finished`, `Unread`) from the cursor the reading client actually holds, using
  `last_completed_seq` and `last_nonterminal_seq`. Both are `#[serde(default)]`.
  `parent` is defaulted and omitted when absent in both the projection and summary, and
  `summary()` copies it from the projection.
- `ProjectionError = WrongTurn(TurnId) | UnknownItem(ItemId) | UnknownGate(GateId) |
  WrongStream(ItemId, StreamKind) | OutOfOrder { expected: Seq, got: Seq }` is the stable
  rejected-transition error.

## `fleet-proto`

`PROTOCOL_VERSION` is **8** in `crates/fleet-proto/src/lib.rs`. The daemon and every client require
an exact Hello match. Version 8 is intentional rather than an additive capability: version 7
required `AgentThreadCreate.mode`, while version 8 omits it when the daemon should resolve the
per-harness default; version 7 also cannot decode the new `auto` and `dont_ask` enum values. Thus
both mixed-version directions fail during Hello with the normal version-mismatch UX, before an
agent request can be misdecoded.

### Requests — `crates/fleet-proto/src/request.rs`

The new `RequestBody` variants are:

```rust
AgentThreadList
AgentSeenCursors
AgentThreadCreate { worktree: WorktreeId, provider: AgentKind,
    model: Option<ModelSelection>, mode: Option<PermissionMode>,
    resume_cursor: Option<String>, title: Option<String> }
AgentThreadOpen { thread: ThreadId, from_seq: Option<Seq>, after_seq: Option<Seq>,
    turn_limit: Option<u32>, before_cursor: Option<String>, request_sync_marker: bool }
AgentItemBody { thread: ThreadId, item: ItemId, stream: StreamKind,
    offset: u64, limit: u32 }
AgentThreadClose { thread: ThreadId }
AgentSend { thread: ThreadId, input: UserInput }
AgentInterrupt { thread: ThreadId }
AgentRespond { thread: ThreadId, gate: GateId, answer: GateAnswer }
AgentSetMode { thread: ThreadId, mode: PermissionMode }
AgentSetModel { thread: ThreadId, model: ModelSelection }
AgentMarkSeen { thread: ThreadId, seq: Seq }
AgentStop { thread: ThreadId }
AgentAccountLogin { thread: ThreadId }
AgentAccountLogout { thread: ThreadId }
AgentCheckpoints { thread: ThreadId }
AgentRevert { thread: ThreadId, checkpoint: CheckpointId }
DelegationRun { caller: ThreadId, provider: AgentKind, brief: String,
    expectation: String, worktree: Option<WorktreeId>, mode: Option<PermissionMode>,
    model: Option<ModelSelection>, title: Option<String>, eager: bool }
DelegationComplete { delegation: DelegationId, child: ThreadId, token: String,
    result: String, blocked: bool }
DelegationList { caller: Option<ThreadId> }
DelegationGet { delegation: DelegationId }
DelegationCancel { delegation: DelegationId }
DelegationWait { delegation: DelegationId, timeout_ms: u64 }
```

`AgentThreadCreate` intentionally carries published `WorktreeId`; the daemon resolves the trusted,
canonical `StartRequest::worktree_path` through its worktree service. An omitted model or mode is
resolved from `config.nativeAgents.<harness>` and the resolved value is persisted. Missing config
sections default each harness to `{ mode: full_access, model: null, effort: null }`.

The five window fields on
`AgentThreadOpen`, plus `AgentItemBody`, are sent only after their capability is negotiated;
`from_seq` retains the legacy cursor meaning and field name. Account and checkpoint mutations are
likewise gated by `agent.account` and `agent.checkpoints`.

The six `Delegation*` request variants are defined in phase 2 and served from phase 3. Their
optional fields and false booleans are defaulted and omitted. `DelegationRun` and
`DelegationComplete` join per-thread serialization under `caller` and `child` respectively;
`DelegationCancel` has no `ThreadId`, so the delegation service serializes it rather than
`agent_request_is_serialized`. On the wire, `RequestBody` uses an internal `type` tag with
snake-case values (`delegation_run` through `delegation_wait`); its fields retain the exact
snake-case spellings above, including `timeout_ms`.

### Responses, events, snapshot

- `crates/fleet-proto/src/response.rs`: `ResponseBody` adds
  `AgentThreads(Vec<AgentThreadSummary>)`, `AgentSeenCursors(Vec<AgentSeenCursor>)`,
  `AgentThreadCreated(AgentThreadSummary)`, `AgentThreadSnapshot { projection:
  ThreadProjection, events_after: Vec<SeqEvent> }`, `AgentThreadWindow(Box<AgentThreadWindow>)`,
  `AgentItemBodyChunk { thread, item, stream, offset, total, text }`, `AgentAck`,
  `AgentAccountLogin { auth_url }`, `AgentCheckpoints(Vec<TurnCheckpoint>)`, and
  `AgentReverted(AgentRevertReport)`.
- The delegation responses are `DelegationStarted { delegation: Delegation, warning:
  Option<String> }`, `Delegations(Vec<Delegation>)`, and `Delegation(Delegation)`. Run uses the
  first, list the second, and complete/get/cancel/wait the third. `warning` is defaulted and
  omitted when absent.
- `crates/fleet-proto/src/event.rs`: `EventKind` adds `Agent` and `AgentSummary`; `Event` adds
  `Agent { thread: ThreadId, event: SeqEvent }`, `AgentSummary(AgentThreadSummary)`,
  `AgentResync { thread, from_seq }`, `AgentSynchronized { thread }`, and `AgentWindow { thread }`.
  `pub type EventBody = Event` is the compatibility name for the event payload enum.
- `Event::DelegationChanged(Delegation)` belongs to the `AgentSummary` subscription family and
  is emitted only when the receiving peer advertises `agent.delegation`.
- `crates/fleet-proto/src/snapshot.rs`: `Snapshot::agent_threads: Vec<AgentThreadSummary>` is
  `#[serde(default)]`, so protocol-v4 snapshot JSON still deserializes.

`AGENT_CAPABILITIES` advertises nine strings in order: `agent.window`, `agent.sync_marker`,
`agent.resync`, `agent.item_body`, `agent.checkpoints`, `agent.codex`, `agent.seen`,
`agent.account`, and `agent.delegation`. `DelegationRun` uses the harness-start client timeout,
and `DelegationWait { timeout_ms, .. }` uses `timeout_ms + 15 seconds` so the transport outlives
the service deadline.

## `fleet-daemon`

### Adapter boundary — `crates/fleet-daemon/src/services/agents/providers/mod.rs`

> **Replaced by the harness rewrite.** The real adapters live in `crates/fleet-daemon/src/agents/`
> behind the `Harness` trait of `NATIVE-AGENTS.md` §3.1; `providers/mod.rs` is now the single
> bridge between that trait and the verbs the manager speaks, and the former provider-specific
> manager adapters are deleted. The current shape is below; the paragraphs after it that discuss the channel and
> `ProviderError` are still accurate.

```rust
/// One normalized event, the provider's own name for the message it was mapped from, and how far
/// behind the harness's own emission clock the frame was read.
pub struct ProviderEvent {
    pub event: AgentEvent,
    pub raw: Option<String>,
    pub emission_skew: Option<std::time::Duration>,
}
impl ProviderEvent { pub fn new(event: AgentEvent, raw: Option<&str>) -> Self; }
impl From<AgentEvent> for ProviderEvent {}          // raw: None, emission_skew: None

pub type ProviderSink = tokio::sync::mpsc::UnboundedSender<ProviderEvent>;
pub type ProviderEvents = tokio::sync::mpsc::UnboundedReceiver<ProviderEvent>;
pub type ProviderResult<T> = Result<T, ProviderError>;

#[async_trait]
pub trait AgentProvider: Send {
    fn kind(&self) -> AgentKind;
    fn capabilities(&self) -> HarnessCapabilities;
    async fn start(&mut self, req: StartRequest) -> ProviderResult<()>;
    // Answers which turn the message landed in and whether it joined a running one.
    async fn send(&mut self, turn: TurnId, input: UserInput) -> ProviderResult<Submitted>;
    async fn interrupt(&mut self, turn: TurnId) -> ProviderResult<()>;
    async fn respond(&mut self, gate: GateId, answer: GateAnswer) -> ProviderResult<()>;
    // Applies what the live process can take and *reports* what a restart would still cost.
    async fn apply_runtime(&mut self, change: RuntimeChange) -> ProviderResult<RuntimeApplied>;
    // Performed by the manager at a turn boundary, never by the adapter.
    async fn restart(&mut self, plan: &RestartPlan, change: &RuntimeChange) -> ProviderResult<()>;
    async fn account(&mut self, op: AccountOp) -> ProviderResult<AccountOutcome>;
    async fn stop(&mut self) -> ProviderResult<()>;
    fn events(&mut self) -> ProviderEvents;
}

pub fn spawn_provider(kind: AgentKind, req: &StartRequest,
    binaries: &fleet_core::config::AgentBinaries) -> anyhow::Result<Box<dyn AgentProvider>>;
```

The adapter answer is declared in `crates/fleet-daemon/src/agents/harness/mod.rs` as
`Submitted = JoinedActive { turn: TurnId } | QueuedNew { turn: TurnId }`. `turn()` returns the
turn from either variant, and `joined_active()` is true only for `JoinedActive`. This replaces the
ambiguous former `{ turn, queued }` shape: joining the active turn is a steer, while `QueuedNew`
means a distinct turn accepted now or queued by the harness.

The channel carries `ProviderEvent` rather than a bare `AgentEvent` because §11 of
`NATIVE-AGENTS.md` names `SeqEvent.raw` — "the provider's own event type name for every stored
event" — as the mitigation for protocol drift, and only the adapter knows that name. It is
*unbounded*: every manager verb holds the thread's operation gate across the adapter call and the
drain has to take that same gate, so a bounded channel that filled during a streaming turn would
deadlock the verb against its own drain. The factory spawns nothing — `AgentProvider::start` owns
the transport, so a launch failure arrives as a typed `ProviderError` the manager turns into the
terminal fallback — and it reads each provider's executable from `config.agentBinaries`, the
command the daemon runs with **no shell**, never from `config.agentCommands`, which is the shell
line a PTY pane types.

`ProviderError = Unavailable { reason: String } | Protocol { message: String } |
Exited { code: Option<i32> } | Timeout { what: String }`. `AgentSessionManager` maps
`Unavailable` to `ErrorKind::Unsupported`, which is the error that names the terminal fallback.
`spawn_provider` builds one `HarnessProvider`, which owns a `Box<dyn Harness>` from
`crate::agents::harness::spawn` — `agents/claude/**` for stream-json over stdio, `agents/codex/**`
for the app-server protocol — and re-frames its events onto the manager's channel. The event
receiver is stable across a restart: the replacement process forwards into the same sink, so the
manager's drain is never re-wired.

### Persistence — `crates/fleet-daemon/src/services/agents/store.rs`

- `AGENT_INDEX_VERSION: u32 = 1` and `AGENT_LOG_VERSION: u32 = 1`.
- `AgentStore::new(PathBuf) -> AgentStore`, `root(&self) -> &Path`,
  `append(&self, ThreadId, &SeqEvent) -> anyhow::Result<()>`,
  `load(&self, ThreadId) -> anyhow::Result<Vec<SeqEvent>>`,
  `truncate_after(&self, ThreadId, Option<Seq>) -> anyhow::Result<()>`,
  `read_index(&self) -> anyhow::Result<AgentIndex>`, and
  `write_index(&self, &AgentIndex) -> anyhow::Result<()>` define the storage boundary.
  `truncate_after` is how a log whose tail the reducer refuses on replay is cut back to the last
  event that reduces, rather than quarantining the whole thread.
- `AgentIndex { version: u32, threads: Vec<AgentThreadRecord> }` is versioned and defaults to an
  empty v1 index.
- `AgentThreadRecord` in `crates/fleet-daemon/src/services/agents/record.rs` is
  `{ thread: ThreadId, parent: Option<ThreadId>, delegation:
  Option<DelegationId>, worktree: WorktreeId, provider: AgentKind, title: String,
  created: DateTime<Utc>, last_activity: DateTime<Utc>, resume_cursor: Option<String>,
  model: Option<ModelSelection>, mode: PermissionMode, last_outcome: Option<TurnOutcome>,
  stop_cause: Option<StopCause> }` and is persisted listing/resume metadata. The three
  delegation-era fields are defaulted and omitted when absent, so an older record still decodes.

Migration slot 3 is named `delegations` in
`crates/fleet-daemon/src/services/agents/store/migrations.rs`. It creates:

```sql
CREATE TABLE delegations (
  id TEXT PRIMARY KEY, token_sha256 TEXT NOT NULL,
  caller_thread TEXT NOT NULL, caller_turn TEXT NOT NULL, caller_item TEXT NOT NULL,
  child_thread TEXT NOT NULL UNIQUE, provider TEXT NOT NULL, depth INTEGER NOT NULL,
  brief TEXT NOT NULL, expectation TEXT NOT NULL, eager INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL, status_payload TEXT, result TEXT, result_source TEXT,
  result_files TEXT, result_elided INTEGER NOT NULL DEFAULT 0,
  nudges INTEGER NOT NULL DEFAULT 0, recoveries INTEGER NOT NULL DEFAULT 0,
  delivery TEXT NOT NULL, delivered_seq INTEGER, delivered_turn TEXT, delivery_reason TEXT,
  headline TEXT, created TEXT NOT NULL, finished TEXT
);
CREATE INDEX idx_delegations_caller ON delegations(caller_thread, created);
CREATE TABLE delegation_outbox (
  id INTEGER PRIMARY KEY, delegation TEXT NOT NULL, action TEXT NOT NULL,
  created TEXT NOT NULL, done TEXT
);
CREATE INDEX idx_delegation_outbox_open ON delegation_outbox(id) WHERE done IS NULL;
ALTER TABLE threads ADD COLUMN parent_thread_id TEXT;
ALTER TABLE threads ADD COLUMN delegation_id TEXT;
ALTER TABLE threads ADD COLUMN stop_cause TEXT;
```

The three `ALTER TABLE` operations use the migration ladder's `PRAGMA table_info` guard, as slot
2 does. Status/delivery values are snake-case, `result_files` is a JSON array, and timestamps use
the database's RFC 3339 encoding. Thread insert/read/list paths carry all three columns, and every
metadata write persists `stop_cause`. `rebuild_thread` and `quarantine_after` never recreate,
delete or truncate delegation records: the delegation is separate durable truth, while only its
status columns are derived from thread events.

### Manager/service — `crates/fleet-daemon/src/services/agents/{manager.rs,thread.rs,mod.rs}`

`pub type AgentService = AgentSessionManager`. `AgentSessionManager::new(AgentStore, BroadcastBus,
Worktrees, Arc<ConfigStore>) -> Self` owns store, event broadcast, trusted worktree lookup, and
the configured provider command lines. `summaries(&self) -> Vec<AgentThreadSummary>` feeds the
global snapshot. The following async methods return `Result<ResponseBody, ProtoError>`:
`list(&self)`, `create(&self, WorktreeId, AgentKind, Option<ModelSelection>, Option<PermissionMode>,
Option<String>, Option<String>)`, `open(&self, ThreadId, Option<Seq>)`, `close(&self, ThreadId)`,
`send(&self, ThreadId, UserInput)`, `interrupt(&self, ThreadId)`,
`respond(&self, ThreadId, GateId, GateAnswer)`, `set_mode(&self, ThreadId, PermissionMode)`,
`set_model(&self, ThreadId, ModelSelection)`, `mark_seen(&self, ThreadId, Seq)`, and
`stop(&self, ThreadId)`. `Services::agents` registers this service and dispatch routes every
request. `thread.rs` holds the per-thread runtime (`ThreadState`, `ThreadRuntime`) behind the
serialized operation gate; it is `pub(super)` and not part of the crate's public surface.

Delegations add `DelegationService`, constructed beside the manager and paired with the one
`DelegationWorker` that owns its wake receiver. It holds the store, manager, broadcast bus,
configuration and worktree resolver, and serves the six capability-gated requests through:

```rust
DelegationService::run(RunRequest) -> Result<ResponseBody, ProtoError>
DelegationService::complete(CompleteRequest) -> Result<ResponseBody, ProtoError>
DelegationService::list(Option<ThreadId>) -> Result<ResponseBody, ProtoError>
DelegationService::get(DelegationId) -> Result<ResponseBody, ProtoError>
DelegationService::wait(DelegationId, u64) -> Result<ResponseBody, ProtoError>
DelegationService::cancel(DelegationId) -> Result<ResponseBody, ProtoError>
```

`run` receives `{ caller, provider, brief, expectation, worktree, mode, model, title, eager }`;
`complete` receives `{ delegation, child, token, result, blocked }`. `new` returns the service and
worker together, `wake_sender` supplies the store's post-commit hook, and `publish_changed` emits
`Event::DelegationChanged`. The worker drains once at start, then on a wake and every retry tick.

The manager's delegation seams are:

- `create_with(CreateOptions)`, where `CreateOptions` adds `parent`, `delegation` and `extra_env`
  to the ordinary worktree/provider/model/mode/resume/title inputs;
- `running_turn(ThreadId) -> Result<Option<TurnId>, ProtoError>` and
  `append_item(ThreadId, TurnId, ItemKind) -> Result<ItemId, ProtoError>`, the three request-path
  verbs used to create and seed the child and its caller row;
- `patch_item(ThreadId, ItemId, ItemPatch, Option<ItemStatus>) -> Result<(), ProtoError>`,
  `record(ThreadId) -> Result<AgentThreadRecord, ProtoError>`, and
  `projection(ThreadId) -> Result<ThreadProjection, ProtoError>`, used by the outbox worker; and
- `delegation_facts(&ThreadProjection, &AgentThreadRecord) -> DelegationFacts`, the pre-event
  snapshot passed into the store transaction.

`apply_event` derives those facts before reducing the event. The store's `project_event` runs the
delegation transition after the ordinary item and turn projections, so the delegation status and
its outbox actions commit with the child or caller event that caused them.

## `fleet-client`

Typed methods live in `crates/fleet-client/src/api/agents.rs`; mirror types live in
`crates/fleet-client/src/api/agents/mirror.rs`. Both are re-exported by `fleet_client`.

- `AgentSnapshot { projection: ThreadProjection, events_after: Vec<SeqEvent> }` is the typed open
  response.
- `Client` methods are `agent_thread_list() -> Result<Vec<AgentThreadSummary>>`,
  `agent_thread_create(WorktreeId, AgentKind, Option<ModelSelection>, Option<PermissionMode>,
  Option<String>, Option<String>) -> Result<AgentThreadSummary>`,
  `agent_thread_open(ThreadId, Option<Seq>) -> Result<AgentSnapshot>`,
  `agent_thread_close(ThreadId) -> Result<()>`, `agent_send(ThreadId, UserInput) -> Result<()>`,
  `agent_interrupt(ThreadId) -> Result<()>`,
  `agent_respond(ThreadId, GateId, GateAnswer) -> Result<()>`,
  `agent_set_mode(ThreadId, PermissionMode) -> Result<()>`,
  `agent_set_model(ThreadId, ModelSelection) -> Result<()>`,
  `agent_mark_seen(ThreadId, Seq) -> Result<()>`, and `agent_stop(ThreadId) -> Result<()>`.
  Every method is async and validates its exact agent response variant.
- `DelegationRunRequest { caller, provider, brief, expectation, worktree, mode, model, title,
  eager }` mirrors `RequestBody::DelegationRun`. `Client` adds
  `delegation_run(DelegationRunRequest) -> Result<(Delegation, Option<String>)>`,
  `delegation_complete(DelegationId, ThreadId, String, String, bool) -> Result<Delegation>`,
  `delegation_list(Option<ThreadId>) -> Result<Vec<Delegation>>`,
  `delegation_get(DelegationId) -> Result<Delegation>`,
  `delegation_cancel(DelegationId) -> Result<Delegation>`, and
  `delegation_wait(DelegationId, u64) -> Result<Delegation>`. All are async and validate the
  response variant described above.
- `MirrorOutcome = Applied | Duplicate { applied: Seq } | Gap { expected: Seq, got: Seq } |
  Rejected { seq: Seq }` reports ordered delivery. `Duplicate` is a replay, which a cursored open
  produces by construction (§4.3) and which no resync repairs; `Rejected` is a continuous event
  the projection refused on its own terms, which a resync cannot repair either — only `Gap` is
  worth a round-trip.
- `AgentMirror { projections: HashMap<ThreadId, ThreadProjection>,
  last_seen: HashMap<ThreadId, Seq> }` exposes mirror state. `apply_event(&mut self, ThreadId,
  &SeqEvent) -> MirrorOutcome` checks continuity then delegates to the projection;
  `install_snapshot(&mut self, ThreadProjection, &[SeqEvent]) -> MirrorOutcome` replaces and
  catches up; `mark_seen(&mut self, ThreadId, Seq)` records the local cursor; and
  `apply_or_resync(&mut self, ThreadId, &SeqEvent, F) -> Result<MirrorOutcome>` applies an event
  and, on a gap only, fetches an ordered tail from the caller's `F: FnOnce(ThreadId, Option<Seq>)
  -> impl Future<Output = Result<AgentSnapshot>>`. A mirror with no projection resyncs from
  `Some(Seq(0))`, never `None`, because the daemon reads a `None` cursor as the §6 lazy resume and
  would relaunch a provider nobody asked for.
- `AgentEvents` is the ordered subscription built by `Client::agent_events() -> AgentEvents`, with
  `mirror(&self) -> &AgentMirror`, `install(&mut self, AgentSnapshot) -> MirrorOutcome`, and
  `recv(&mut self) -> Result<(ThreadId, SeqEvent)>`, which repairs gaps through `apply_or_resync`
  before it returns. It has **no consumer today** and is not re-exported from the crate root:
  `fleet agent tail` drives `Client::events()` and its own cursored re-open, and `fleet-app`
  mirrors through `state/agents.rs`. Treat it as a convenience seam, not a shipped guarantee —
  either give it a consumer and a `pub use` in `lib.rs`, or drop it.

## `fleet-ui-kit`

- `components/multiline_input/` (`history.rs`, `input.rs`, `triggers.rs`):
  `MultilineInputEvent = Submit(String) | Trigger(Trigger) | Changed | Escape`;
  `HISTORY_LIMIT: usize = 100`; `MULTILINE_INPUT_KEY_CONTEXT: &str = "FleetMultilineInput"`, the
  key context the composer pushes. The app binds submit, history and escape in its own agent
  contexts rather than shadowing them with `gpui::NoAction`; the nested `FleetTextInput` consumes
  the editing actions and propagates vertical motion at visual-row boundaries. `MultilineInput`
  implements `EventEmitter`, `Focusable` and `Render`, with
  `new(&mut Context<Self>, SharedString) -> Self`, `text(&self, &App) -> &str`,
  `is_empty(&self, &App)`, `is_composing(&self, &App)`, `buffer(&self, &App) -> &InputBuffer`,
  `active_trigger(&self, &App) -> Option<Trigger>`, `set_text`, `clear`, `set_placeholder`,
  `set_focus_visible`, `set_read_only`, `focus_handle`, `push_history`, `submit`,
  `replace_and_mark_text_in_range`, `recall_previous(&mut self, &mut Context<Self>) -> bool`, and
  `caret_up` / `caret_down`, both `-> bool` so an owner can fall through to prompt history.
  The composer owns an `Entity<TextInput>` (ADR 0019): editing, selection, undo, IME, pointer
  geometry and scrolling belong to that shared input, and `InputBuffer` — the editable text model
  with selection, word motion and UTF-16 offsets for IME — lives in `components/input/buffer.rs`.
  This wrapper keeps only prompt history, completion triggers, submit/escape events, read-only
  state and focus-visible chrome. `triggers.rs` owns `Trigger` and `PromptHistory` is the bounded
  submitted-prompt ring in `history.rs`.
- `components/markdown/` (`parser.rs`, `render.rs`, `code.rs`):
  `MarkdownDocument { blocks: Vec<MarkdownBlock> }`;
  `MarkdownBlock = Paragraph(Vec<MarkdownInline>) | Code { lang: Option<String>,
  text: SharedString, highlights: CodeHighlights } (built by `MarkdownBlock::code(lang, text)`) |
  List { ordered: bool, items: Vec<Vec<MarkdownBlock>> } | Heading { level: u8,
  inlines: Vec<MarkdownInline> } | Quote(Vec<MarkdownBlock>) | Rule`;
  `MarkdownInline = Text(String) | Code(String) | Strong(Vec<MarkdownInline>) |
  Emphasis(Vec<MarkdownInline>) | Link { label: Vec<MarkdownInline>, url: String }`.
  `parse_markdown_document(&str) -> MarkdownDocument` and
  `markdown(&MarkdownDocument, &App) -> impl IntoElement`.
- `components/agent/metrics.rs`: `AGENT_CONTENT_W 760` · `AGENT_TOOL_KIND_W 60` ·
  `AGENT_CARET_H 17` · `AGENT_BODY_MAX_H 240` · `AGENT_LIST_OVERDRAW 256` ·
  `AGENT_SCROLLBAR_INSET 3`, all `Pixels`. They are constants rather than theme tokens because
  §2 fixes them; see `DESIGN-SYSTEM.md` §2.
- `components/agent/format.rs`: `MINUS: char` plus `format_duration`, `format_token_count`,
  `format_file_delta`, `format_files_changed`, `format_turn_footer`, `format_worked`,
  `format_thinking`, `format_retrying`, `format_compacted`, `format_resumed`, each returning
  `SharedString`. Every §2 string a transcript row shows is built here, so the CLI, the tab and
  the gallery cannot word the same fact differently.
- `components/agent/tool_row.rs`: `ToolRowState = Running | Done | Error | Denied` with
  `glyph(self) -> (Icon, Tone)` and `is_running(self) -> bool`;
  `ToolRow { id, state, kind, summary, result, output, diff, expanded }` built with
  `ToolRow::new(id, kind, summary)` and the `state`/`result`/`output`/`diff`/`expanded` builders,
  plus `has_body(&self) -> bool`; `tool_row(&ToolRow, &App) -> impl IntoElement` for a static row
  and `ToolRowElement::new(ToolRow)` with `focused`, `body`, `children` and `on_toggle` for an
  interactive one; `expand_hint(expanded: bool) -> KeyHint`.
- `components/agent/decision_card.rs`: `DecisionAction = AllowOnce | AllowSession |
  AllowDirectory | Deny | Edit | DenyAndStop | Choose(usize) | Toggle | Answer | ApprovePlan |
  AskForChanges | ViewPlan`. `DecisionQuestion { header, text, options, multi_select, allow_other }`
  carries one question's presentation; `SOMETHING_ELSE` is the free-text option's label.
  `DecisionCardKind = Permission { tool, payload, rationale } | Question { questions } |
  Plan { markdown, steps }`; `DecisionOption { key, label, action }` with
  `DecisionOption::new(key, label, action)`; and
  `DecisionCard { id, title, kind, actions, expanded, selected, cursor }` with
  `DecisionCard::new(id, title, kind)`, the `expanded` / `cursor` / `selected` / `actions`
  builders, `option_count(&self, usize) -> usize` and
  `action_for_key(&self, &str) -> Option<DecisionAction>` — which honours only the digits the
  question under `cursor` actually offers. `selected` and `cursor` travel with the card because
  the row is rebuilt from the owner's in-progress answer every frame; without them `1`–`4` and
  `space` would change state the user cannot see. `permission_actions`, `question_actions` and
  `plan_actions` build the standard key rows, `decision_card(&DecisionCard, &App)` draws a static
  card, `DecisionCardElement::new(DecisionCard)` (with `default_action`, `selected`, `expanded`,
  `answer`, `on_action`) an interactive one, and `decision_key_hints(&DecisionCard) -> KeyHintRow`
  the footer legend. These UI-kit types mirror, rather than import,
  `fleet_core::agents::GateKind` to preserve dependency direction.
- `components/agent/transcript_list.rs`: `TranscriptRow = UserBlock { text, attachments } |
  AssistantText { markdown } | Thinking { text, duration_ms, expanded } |
  ToolRow { row, children } | WorkedFold { text, expanded } | TurnFooter { text } |
  DecisionCard(DecisionCard) | ErrorCard { message, retrying } | CheckpointLine { text } |
  Notice { text } | QueuedMessage { text } | EmptyState { message }`, with
  `id(&self) -> Option<SharedString>` and `is_expandable(&self) -> bool`.
  `TranscriptEvent = Toggle(SharedString) | Decision { card, action }` is what the list asks its
  owner to do. `RowSplice { old_range, count }` and
  `diff_rows(&[TranscriptRow], &[TranscriptRow]) -> Option<RowSplice>` keep a streaming rebuild to
  one contiguous splice; `scroll_fraction(Pixels, Pixels, Pixels) -> Option<(f32, f32)>` sizes the
  scrollbar; `ToolBodyRenderer = Rc<dyn Fn(&ToolRow, &mut App) -> Option<AnyElement>>` lets the
  app inject the inline diff without the kit depending on it. `TranscriptList` implements `Render`
  and exposes `new(&mut Context<Self>)`, `set_tool_body`, `set_rows`, `scroll_to_bottom`,
  `scroll_to_end`, `scroll_mode(bool, …)`, `is_scroll_mode`, `scroll_rows(f32, …)`,
  `scroll_viewports(f32, …)`, `scroll_to_top`, `is_at_bottom`, `set_streaming`, `focus_row`,
  `focused_row`, `rows(&self) -> &[TranscriptRow]`, `focus_handle`,
  `open_decision(&self) -> Option<&DecisionCard>` and
  `event_for_key(&self, &str) -> Option<TranscriptEvent>`. Free helpers `user_block`,
  `user_block_with`, `attachment_pill` and `error_card` draw those rows outside a list.
- `theme/tokens.rs`: `ColorTokens::diff_added` and `diff_removed` are semantic diff tints.
  Existing `success`, `warning`, `danger`, `focus`, background/surface/elevated/border, and
  primary/muted/subtle text fields cover the remaining agent semantic tokens; values are recorded
  in `docs/DESIGN-SYSTEM.md`.
- `components/mode_word.rs`: `Mode::Agent` is the `AGENT` status word.
- `components/terminal_tab_strip.rs`: `TerminalTab::unread(bool)` is the neutral dot an agent tab
  draws when content arrived while the user was elsewhere; amber `activity` wins over it.
- `examples/gallery_agent.rs` exercises the transcript, tool rows, decision cards and composer.

## `fleet-lazygit`

`crates/fleet-lazygit/src/diff_view.rs` owns the reusable domain-neutral diff surface and
`fleet_lazygit::diff_view` exports it. `DiffView` implements `Render` and has
`new(impl Into<SharedString>, &mut Context<Self>) -> Self`,
`for_path(Option<impl Into<SharedString>>, impl Into<SharedString>, &mut Context<Self>)` —
the path is what picks the grammar, which a `ToolDiff` needs because its `unified` text may carry
no `---` header — `unified(&self) -> &str`,
`set_unified(&mut self, impl Into<SharedString>, &mut Context<Self>)`,
`expanded(&self) -> bool` / `set_expanded(&mut self, bool, &mut Context<Self>)`, and
`set_actions(impl Fn(&App) -> AnyElement + 'static, …)` / `clear_actions` for the key row under
the rows. `MAX_ROWS: usize = 400` bounds an inline diff. It uses the ADR 0005 lazygit rendering
stack, so `fleet-ui-kit` gains no `fleet-git` dependency: the payload rows come from the extracted
`crates/fleet-lazygit/src/views/row_layout.rs`, which lazygit's own full-window
`views/diff.rs` draws from too, so an inline diff and a full-window diff keep the same geometry.

## `fleet-app`

- `bridge.rs`: `BridgeEvent` adds `Agent { thread: ThreadId, event: SeqEvent }` and
  `AgentSummary(AgentThreadSummary)`, plus seen-cursor and delegation census refreshes.
  `BridgeCommand` mirrors the complete native-agent `RequestBody` command family and converts
  losslessly with `From<BridgeCommand> for RequestBody`.
  `Bridge::send_agent(BridgeCommand)` is fire-and-forget and
  `Bridge::request_agent(BridgeCommand) -> Receiver<Result<ResponseBody, ProtoError>>` is typed.
- `actions.rs`: module `native_agent` declares `Send`, `Steer`, `SendBackground`, `PlanMode`,
  `Commands`, `Skills`, `Files`, `History`, `HistoryNext`, `Model`, `Traits`, `AccessMode`,
  `Scroll`, `ScrollLineDown`, `ScrollLineUp`,
  `ScrollHalfPageDown`, `ScrollHalfPageUp`, `ScrollPageDown`, `ScrollPageUp`, `ScrollTop`,
  `ScrollBottom`, `ScrollExit`, `NewClaude`, `NewCodex`, `Stop`, `AllowOnce`,
  `AllowSession`, `Deny`, `EditCommand`, `DenyAndStop`, `Choose1`, `Choose2`, `Choose3`,
  `Choose4`, `Choose5`, `Toggle`, `Answer`, `Previous`, `Implement`, `Refine`, `ExpandRow`,
  `AttachChild`, `Revert`, `OpenInEditor`, `CopyRow`, `DiffRow`, `CancelDelegation`, `CloseTab`,
  `TerminalFallback`, `SelectTab1` through `SelectTab9`, `LastTab`, and `LastSession`. There is no
  `Newline` action: `⇧⏎` belongs to the composer, and binding it here consumed the key from the
  buffer that already owns it.
- `keymap.rs`: the root `Agent` context has `AgentIdle`, `AgentWorking`, `AgentNativeScroll` and
  `AgentDecision`; decision routing uses child contexts `AgentPermission`, `AgentQuestion`, and
  `AgentPlan` to avoid key collisions. Focused transcript rows use `AgentRow`. `/`, `@` and `⇧⏎`
  are deliberately unbound. Bindings are exactly the §9 keys; `KEYMAP.md` is authoritative.
- `screens/agent_thread/` (`mod.rs`, `rows.rs`, `decisions.rs`, `presentation.rs`, `picker.rs`):
  `AgentThreadView` owns `ThreadId`, `ThreadProjection`, `Entity<TranscriptList>`, and
  `Entity<MultilineInput>`. It implements `Render` and exposes
  `new(ThreadProjection, &mut Context<Self>) -> Self`, `thread(&self) -> ThreadId`,
  `projection(&self) -> &ThreadProjection`, `set_projection(&mut self, ThreadProjection,
  &mut Context<Self>)`, `transcript(&self) -> &Entity<TranscriptList>`, and
  `input(&self) -> &Entity<MultilineInput>`. It issues no I/O: every mutation leaves as
  `AgentThreadEvent = Command(BridgeCommand) | OpenInEditor(String) | Notice(SharedString)`
  (crate-internal), which `screens/workspace/agent.rs` relays.
- `state/agents.rs`: `AgentThreads` is the client mirror — daemon summaries, opened
  `ThreadProjection`s, per-worktree selected tab, seen cursors, pending resyncs — with
  `summaries`, `of_worktree`, `summary`, `projection`, `seen`, `attention`, `counts`, `active`,
  `activate`, `deactivate`, `install_snapshot`, `apply_event`, `apply_summary`, `mark_seen`,
  `stale_seen`, `sync_snapshot` and `attention_edges`. `AgentCounts { needs_you, working, failed }`
  with `any(self) -> bool` backs the three status-bar chips. `AppState` adds
  `active_agent_thread()`, `agent_context_chain()`, `apply_agent_event()` and
  `apply_agent_summary()`.
- `shell/root/focus.rs`: the private `FocusTarget` gains `AgentThread`, the case where the agent
  tab's own view owns the keyboard and the shell must not steal focus back to the body.

## `fleet-cli`

`fleet agent` is a subcommand group in `crates/fleet-cli/src/args.rs`, implemented in
`commands/agents.rs`: `list`, `new <WORKTREE> --provider <claude|codex> [--model M]
[--mode ask|accept-edits|plan|auto|dont-ask|full-access]`, `send <THREAD> <TEXT>`,
`respond <THREAD> <GATE> <ANSWER…>`, `interrupt <THREAD>`, `stop <THREAD>`,
`tail <THREAD> [--replay]`, and `terminal [claude|codex]` — the last being the former
`fleet agent [claude|codex]`, kept under its own verb as the §10 PTY fallback. Read-only verbs
open with `Some(Seq(0))` so looking at a thread never triggers the §6 lazy resume.

`fleet subagent` is implemented in `commands/subagents.rs`. Every verb accepts `--json`; human
output otherwise follows the exact copy in `NATIVE-AGENTS.md` §15.

- `run --provider <claude|codex> [--brief-file F] --expect <text> [--worktree W] [--mode M]
  [--model M] [--title T] [--eager] [--caller <thread>]` reads the brief from the file or stdin.
  Caller selection is `--caller`, then `FLEET_SESSION`; neither being present is a validation
  error.
- `complete [<id>] [--result-file F] [--blocked] [--json-result]` reads the result from the file
  or stdin. Its id is the argument or `FLEET_DELEGATION`; its child is `FLEET_SESSION`; its bearer
  token is `FLEET_DELEGATION_TOKEN`. All three are required after fallback. `--json-result`
  validates the result as JSON but does not change its wire type.
- `wait <id> [--timeout S]` defaults to 540 seconds and caps the flag at 540; timeout exits 2 and
  a terminal record exits 0.
- `status <id>` prints one delegation.
- `list [--caller T]` prints all delegations or those belonging to one caller.
- `cancel <id>` cancels one live delegation.

The environment contract is therefore deliberately narrow: `FLEET_SESSION` is the `run` caller
fallback and the mandatory `complete` child; `FLEET_DELEGATION` is the `complete` id fallback;
and `FLEET_DELEGATION_TOKEN` authenticates `complete`. No other subagent verb reads delegation
environment state.

## Additions after the Stage 0 freeze

Nothing in the freeze was renamed or removed; the fix rounds added the following, and the sections
above are the current truth.

| Where | Added |
| --- | --- |
| `fleet-core` events | `AgentEvent::MetadataChanged { title, mode, model }` |
| `fleet-core` projection | `ThreadProjection::{last_completed_seq, last_nonterminal_seq}`, `ThreadProjection::accepts`, `TurnFooter`, `TurnRecord::footer`, `AgentThreadSummary::{last_completed_seq, last_nonterminal_seq, attention_for}` |
| `fleet-daemon` providers | `ProviderEvent::new`, `exit_code`, `ProviderSink`, unbounded `ProviderEvents`, `spawn_provider(kind, &StartRequest, &AgentBinaries)` |
| `fleet-daemon` store | `AGENT_LOG_VERSION`, `AgentStore::truncate_after` |
| `fleet-daemon` manager | `AgentSessionManager::new` takes `Arc<ConfigStore>`; `thread.rs` holds the per-thread runtime |
| `fleet-client` | `MirrorOutcome::{Duplicate, Rejected}`, `install_snapshot(…, &[SeqEvent])`, `AgentMirror::apply_or_resync`, `AgentEvents` and `Client::agent_events` |
| `fleet-ui-kit` agent | `components/agent/{format,metrics}.rs`, `ToolRowElement`, `DecisionCardElement`, `decision_key_hints`, `permission_actions`/`question_actions`/`plan_actions`, `SOMETHING_ELSE`, `expand_hint` |
| `fleet-ui-kit` transcript | `TranscriptEvent`, `RowSplice`/`diff_rows`, `scroll_fraction`, `ToolBodyRenderer`, `TranscriptRow::Notice`, `UserBlock.attachments`, `ErrorCard.retrying`, `DecisionCard::{selected, cursor}` |
| `fleet-ui-kit` elsewhere | `InputBuffer`/`PromptHistory`/`MULTILINE_INPUT_KEY_CONTEXT`, `Mode::Agent`, `TerminalTab::unread`; the diff tints live in `theme/tokens.rs` on `ColorTokens`, not on a `ThemeColors` |
| `fleet-lazygit` | `DiffView::{for_path, expanded, set_expanded, set_actions, clear_actions}`, `MAX_ROWS` |
| `fleet-app` | `native_agent::{HistoryNext, ScrollLineDown, ScrollLineUp, ScrollHalfPageDown, ScrollHalfPageUp, ScrollPageDown, ScrollPageUp, ScrollTop, ScrollBottom, ScrollExit, TerminalFallback}` (and no `Newline`), the `AgentNativeScroll` key context, `AgentThreadEvent`, `state/agents.rs`, `FocusTarget::AgentThread` |
| `fleet-cli` | `fleet agent terminal` |

## Additions made by the native-agents rewrite

What the rewrite added on top of the sections above, in one table so a reader holding the previous
revision can find it. Everything here is additive on the wire and defaulted where it is serialized.

| Where | Added |
| --- | --- |
| `fleet-core` provider input | `UserInput.item: Option<ItemId>` — the identity the client already drew its optimistic bubble under; both adapters adopt it |
| `fleet-core` items | `ItemKind::UserMessage.steered: bool` and `ItemPayloadPatch::UserMessage.steered: Option<bool>` — the harness's own answer to "did this join a running turn?" |
| `fleet-proto` handshake | `HelloClient.capabilities: Vec<String>` + `HelloClient::supports`; `AGENT_CAPABILITIES` is published by both sides |
| `fleet-proto` agents | `elided_stream(&ItemKind) -> Option<StreamKind>` — the one function both the daemon's narrowing pass and the client's `AgentItemBody` read agree on |
| `fleet-proto` snapshot | `AgentBinaries.codex: bool`, defaulted |
| `fleet-daemon` harness | `crates/fleet-daemon/src/agents/**`: the `Harness` trait, `harness::spawn`, the probe cache, `SchemaFingerprint`, and the two adapters |
| `fleet-daemon` manager | `manager/{bodies,checkpoints,controls}.rs`; `AgentSessionManager::{item_body, set_checkpoints}`; `pending_inputs` (was `pending_claude_inputs`, now used by both harnesses) |
| `fleet-ui-kit` transcript | `TranscriptEvent::ReachedOldest`, `OLDEST_PREFETCH_ROWS` |
| `fleet-app` | `BridgeCommand::{AgentCheckpoints, AgentRevert}`; `AgentThreadEvent::{RefreshCheckpoints, LoadOlder}`; `AgentThreadView::install_checkpoints` |

## File ownership during implementation

| Stage | Exclusive behavior implementation area |
|---|---|
| B1 | `crates/fleet-core/src/agents/projection.rs` |
| B2 | `crates/fleet-daemon/src/services/agents/providers/claude/**` |
| B3 | `crates/fleet-daemon/src/services/agents/providers/opencode/**` |
| B4 | `crates/fleet-daemon/src/services/agents/{manager.rs,store.rs,thread.rs}` plus agent dispatch/service integration |
| B5 | `crates/fleet-ui-kit/src/components/multiline_input/**` |
| B6 | `crates/fleet-ui-kit/src/components/markdown/**` |
| B7 | `crates/fleet-ui-kit/src/components/agent/**` |
| B8 | `crates/fleet-lazygit/src/diff_view.rs` |
| B9 | Native-agent seams under `crates/fleet-app/src/{bridge.rs,actions.rs,keymap.rs,state/agents.rs,screens/agent_thread/**,screens/workspace/agent.rs}` |

Shared contracts in `fleet-core::agents` and `fleet-proto` are frozen integration surfaces. Changes
there require coordination across all consumers; existing terminal-agent popup files remain outside
the native-agent ownership map.
