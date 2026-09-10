<!-- Captured live from the installed `codex app-server`. Regenerate the schema with:
     codex app-server generate-ts          --out <dir> --experimental
     codex app-server generate-json-schema --out <dir> --experimental -->

# Headless wire protocol: Codex app-server 0.147.0

Authority over: the Codex app-server JSON-RPC surface Fleet's adapter is written against.
`NATIVE-AGENTS.md` §4.2 fixes the decisions; this file is the reference.

Verified 2026-09-10 against:

- `codex --version` -> `codex-cli 0.147.0`.
- The generated bindings for that exact binary: 625 types in the `v2` namespace, produced by
  `codex app-server generate-ts --out <dir> --experimental`.
- A live driven session over stdio: handshake, one ordinary turn with a shell tool call, and one
  real failure. Frame sequences below are transcribed from that capture.

**This file outranks any vendored or third-party schema**, including the copy in t3code's
`packages/effect-codex-app-server/src/_generated/`, which is a snapshot of whatever version that
project pinned. Regenerate against the installed binary whenever the CLI is upgraded and diff this
document against the result.

Names and discriminants are case-sensitive. The protocol evolves **additively and without a
version bump** -- `rateLimitExceeded` and `misalignmentPolicyViolation` were both added to
`CodexErrorInfo` after a pinned schema -- so ignore unknown fields and variants and never
reinterpret them. `NATIVE-AGENTS.md` §4.5 carries the tolerant-decode rules this implies.

## 1. Transport

`codex app-server` speaks JSON-RPC 2.0 over stdio by default (`--listen stdio://`, also
`unix://PATH` and `ws://IP:PORT`). It is **bidirectional**: the server issues requests to the
client, which is how approvals and model questions arrive.

Three envelope kinds:

- `ClientRequest` — client → server, id-correlated. 120 methods.
- `ServerRequest` — server → client, id-correlated, **client must answer**. 11 methods.
- `ServerNotification` — server → client, fire-and-forget. 71 methods.

`serverRequest/resolved` is a notification telling the client a `ServerRequest` it was shown no
longer needs an answer — the Codex equivalent of Claude's `control_cancel_request`. A gate must
close on it without sending a response.

## 2. The methods Fleet actually needs

Of the 120 client requests, Fleet's adapter needs roughly these. The rest are account, plugin,
marketplace, MCP admin, realtime voice, Windows sandbox, remote control, and fs/process
surfaces that Fleet either owns itself or does not want.

| Method | Purpose |
| --- | --- |
| `thread/start` | create a thread; returns `ThreadStartResponse` with the resolved model, effort, sandbox, approval policy |
| `thread/resume` | reattach by `threadId`, optionally with `path` to a rollout file and a first page of turns |
| `thread/read`, `thread/list`, `thread/loaded/list` | metadata |
| `thread/items/list` | **paginated item history** (`cursor`, `limit`, `sortDirection`) |
| `thread/turns/list` | **paginated turn history** → `TurnsPage { data, nextCursor, backwardsCursor }` |
| `thread/settings/update` | change model, effort, approval policy, sandbox, personality **for subsequent turns** |
| `thread/compact/start` | explicit compaction |
| `thread/fork`, `thread/rollback` | branch / rewind |
| `thread/unsubscribe` | stop receiving notifications for a thread without killing it |
| `turn/start` | submit a turn; per-turn overrides for model/effort/sandbox/approval |
| `turn/steer` | **first-class steering** into a running turn, guarded by `expectedTurnId` |
| `turn/interrupt` | abort the running turn |
| `model/list` | the model catalogue, with per-model `supportedReasoningEfforts` |
| `permissionProfile/list`, `collaborationMode/list`, `skills/list` | control vocabularies |

Note `thread/items/list` and `thread/turns/list`: **Codex is itself a paginated transcript
store.** Fleet must decide deliberately whether its SQLite store is the system of record or a
cache in front of Codex's rollout files. `NATIVE-AGENTS.md` §8 settles it: **Fleet's SQLite store
is the system of record**, because Claude offers no equivalent and the two harnesses must present
one transcript model. Codex readback is cold-start rehydration only.

## 3. Server → client requests (the gates)

These are the only inbound calls the client MUST answer. Each carries `threadId`, `turnId`,
`itemId`, and `startedAtMs`.

```ts
type ServerRequest =
  | { method: "item/commandExecution/requestApproval", id, params: CommandExecutionRequestApprovalParams }
  | { method: "item/fileChange/requestApproval",       id, params: FileChangeRequestApprovalParams }
  | { method: "item/tool/requestUserInput",            id, params: ToolRequestUserInputParams }
  | { method: "item/permissions/requestApproval",      id, params: PermissionsRequestApprovalParams }
  | { method: "mcpServer/elicitation/request",         id, params: McpServerElicitationRequestParams }
  | { method: "item/tool/call",                        id, params: DynamicToolCallParams }
  | { method: "account/chatgptAuthTokens/refresh",     id, params: ... }
  | { method: "attestation/generate",                  id, params: ... }
  | { method: "currentTime/read",                      id, params: ... }
  | { method: "applyPatchApproval",                    id, params: ... }   // legacy v1
  | { method: "execCommandApproval",                   id, params: ... }   // legacy v1
```

The last two are the **v1 legacy** approval pair; the `item/*` forms are v2. Fleet targets v2
and treats the legacy pair as a version-gate failure.

### 3.1 Command execution approval

```ts
type CommandExecutionRequestApprovalParams = {
  threadId, turnId, itemId, startedAtMs
  approvalId?: string | null          // non-null only for zsh-exec-bridge subcommands sharing one itemId
  environmentId: string | null
  reason?: string | null              // e.g. "needs network access"
  networkApprovalContext?: NetworkApprovalContext | null
  command?: string | null
  cwd?: string | null
  commandActions?: Array<CommandAction> | null     // Codex's own parse of the command
  additionalPermissions?: AdditionalPermissionProfile | null
  proposedExecpolicyAmendment?: ExecPolicyAmendment | null
  proposedNetworkPolicyAmendments?: Array<NetworkPolicyAmendment> | null
  availableDecisions?: Array<CommandExecutionApprovalDecision> | null   // ORDERED, client-presentable
}

type CommandExecutionApprovalDecision =
  | "accept" | "acceptForSession"
  | { acceptWithExecpolicyAmendment: { execpolicy_amendment } }
  | { applyNetworkPolicyAmendment:   { network_policy_amendment } }
  | "decline" | "cancel"

type CommandExecutionRequestApprovalResponse = { decision: CommandExecutionApprovalDecision }
```

**`availableDecisions` is authoritative and ordered.** Fleet renders exactly the options Codex
offers, in Codex's order, and never invents an "always allow" that this request did not offer.
This is a real divergence from Claude, where the option set is derived from
`permission_suggestions`.

`approvalId` matters: one `itemId` can carry several concurrent approval callbacks. The gate key
is `(itemId, approvalId)`, not `itemId`.

### 3.2 File change approval

```ts
type FileChangeRequestApprovalParams = {
  threadId, turnId, itemId, startedAtMs
  reason?: string | null
  grantRoot?: string | null      // [UNSTABLE] allow writes under this root for the session
}
type FileChangeApprovalDecision = "accept" | "acceptForSession" | "decline" | "cancel"
```

Note what is **absent**: the patch itself. The diff arrives separately on the `fileChange` item
(`item/fileChange/patchUpdated`, and the item's `changes: FileUpdateChange[]`). The card must
join the approval request to the item it names by `itemId`.

### 3.3 Model questions — `item/tool/requestUserInput`

Codex has a native equivalent of Claude's `AskUserQuestion`:

```ts
type ToolRequestUserInputParams = {
  threadId, turnId, itemId
  questions: Array<ToolRequestUserInputQuestion>
  isBlocking: boolean
  autoResolutionMs: number | null   // @deprecated — use isBlocking
}
type ToolRequestUserInputQuestion = {
  id: string, header: string, question: string,
  isOther: boolean,      // a free-text "something else" choice is allowed
  isSecret: boolean,     // answer must be masked and never logged
  options: Array<{ label: string, description: string }> | null   // null ⇒ pure free text
}
type ToolRequestUserInputResponse = { answers: { [questionId]: { answers: string[] } } }
```

Two things Claude does not have and Fleet must not flatten away:

- **`isSecret`** — the answer is a credential. Fleet must mask the input, keep it out of the
  transcript, out of the SQLite store, and out of every log line.
- **`isBlocking: false`** — a non-blocking question the turn will proceed past. Fleet must not
  mark the thread `NeedsYou` for one, and must keep it answerable after the turn settles.

Answers are keyed by **question id**; Claude's are keyed by the exact question *text*.

### 3.4 Permission profile approval

`item/permissions/requestApproval` carries `permissions: RequestPermissionProfile` and `cwd` —
the agent asking to widen its sandbox mid-turn. Claude has no counterpart.

## 4. Server → client notifications that build the transcript

Grouping the 71 notifications by what Fleet does with them:

**Thread lifecycle** — `thread/started`, `thread/status/changed`, `thread/closed`,
`thread/name/updated`, `thread/settings/updated`, `thread/tokenUsage/updated`,
`thread/compacted`, `thread/environment/{connected,disconnected}`.

```ts
type ThreadStatus =
  | { type: "notLoaded" } | { type: "idle" } | { type: "systemError" }
  | { type: "active", activeFlags: Array<ThreadActiveFlag> }
type ThreadActiveFlag = "waitingOnApproval" | "waitingOnUserInput"
```

**`activeFlags` hands Fleet its attention state directly** — no inference needed. Claude requires
Fleet to derive the same thing from open gates.

**Turn lifecycle** — `turn/started`, `turn/completed`, `turn/diff/updated`, `turn/plan/updated`,
`turn/moderationMetadata`.

```ts
type Turn = { id, items: ThreadItem[], itemsView, status: TurnStatus,
              error: TurnError | null, startedAt, completedAt, durationMs }
type TurnStatus = "completed" | "interrupted" | "failed" | "inProgress"
```

`turn/completed` carries the **whole settled Turn**, including `status`, `error` and
`durationMs`. It is the completion authority, exactly analogous to Claude's single `result`.
`turn/diff/updated` gives Fleet a turn-level unified diff for free — Claude has no equivalent
and Fleet must compute it from edit items.

**Items** — `item/started`, `item/completed`, plus per-kind deltas:

| Notification | Streams into |
| --- | --- |
| `item/agentMessage/delta` | assistant prose |
| `item/reasoning/summaryTextDelta` (+ `summaryPartAdded`) | reasoning **summary** |
| `item/reasoning/textDelta` | reasoning **raw text** |
| `item/plan/delta` | the plan item |
| `item/commandExecution/outputDelta` | shell output |
| `item/commandExecution/terminalInteraction` | PTY interaction |
| `item/fileChange/outputDelta`, `item/fileChange/patchUpdated` | the patch |
| `item/mcpToolCall/progress` | MCP progress |
| `item/autoApprovalReview/{started,completed}` | the guardian's auto-approval review |

Reasoning arrives on **two channels**: `summary` (a redacted, user-facing summary, streamed per
`summaryIndex`) and `content` (raw reasoning text, only when the model and account permit it).
`ThreadItem.reasoning` carries both as `summary: string[]` and `content: string[]`. Claude has
one channel (`thinking_delta`). Fleet renders the summary by default and keeps raw behind the
same fold.

**Errors and advisories** — `error` (`ErrorNotification { error: TurnError, willRetry, threadId,
turnId }` — note **`willRetry`**, so a retry is not a failure), `warning`, `guardianWarning`,
`deprecationNotice`, `configWarning`, `model/rerouted`, `model/verification`,
`model/safetyBuffering/updated`, `windows/worldWritableWarning`.

```ts
type CodexErrorInfo =
  | "contextWindowExceeded" | "sessionBudgetExceeded" | "usageLimitExceeded"
  | "serverOverloaded" | "cyberPolicy" | "internalServerError" | "unauthorized"
  | "badRequest" | "threadRollbackFailed" | "sandboxError"
  | { httpConnectionFailed: { httpStatusCode } }
  | { responseStreamConnectionFailed: { httpStatusCode } }
  | { responseStreamDisconnected: { httpStatusCode } }
  | { responseTooManyFailedAttempts: { httpStatusCode } }
  | { activeTurnNotSteerable: { turnKind: NonSteerableTurnKind } }
  | "other"
```

This is a **typed** error taxonomy. Fleet should map it to structured errors rather than a
string, and in particular `contextWindowExceeded` / `usageLimitExceeded` /
`activeTurnNotSteerable` deserve distinct UI.

## 5. The item vocabulary — `ThreadItem`

This is Codex's answer to "what blocks can appear in a transcript".

```ts
type ThreadItem =
  | { type: "userMessage", id, clientId: string|null, content: UserInput[] }
  | { type: "hookPrompt", id, fragments: HookPromptFragment[] }
  | { type: "agentMessage", id, text, phase: MessagePhase|null, memoryCitation: MemoryCitation|null }
  | { type: "plan", id, text }
  | { type: "reasoning", id, summary: string[], content: string[] }
  | { type: "commandExecution", id, pluginId, scriptPath, command, cwd, processId,
      source: CommandExecutionSource, status: CommandExecutionStatus,
      commandActions: CommandAction[], aggregatedOutput, exitCode, durationMs }
  | { type: "fileChange", id, changes: FileUpdateChange[], status: PatchApplyStatus }
  | { type: "mcpToolCall", id, server, tool, status, arguments, appContext,
      pluginId, readOnlyHint, result, error, durationMs }
  | { type: "dynamicToolCall", id, namespace, tool, arguments, status, contentItems, success, durationMs }
  | { type: "collabAgentToolCall", id, tool, status, senderThreadId, receiverThreadIds,
      prompt, model, reasoningEffort, agentsStates }
  | { type: "subAgentActivity", id, kind: "started"|"interacted"|"interrupted", agentThreadId, agentPath }
  | { type: "webSearch", ... } | { type: "imageView", id, path }
  | { type: "sleep", ... } | { type: "imageGeneration", ... }
  | { type: "enteredReviewMode", id, review } | { type: "exitedReviewMode", id, review }
  | { type: "contextCompaction", id }
```

Status enums are shared and small:

```ts
CommandExecutionStatus = "inProgress" | "completed" | "failed" | "declined"
PatchApplyStatus       = "inProgress" | "completed" | "failed" | "declined"
```

`"declined"` is a first-class terminal status — a denied tool is not an error, and Fleet's item
status enum needs the same distinction.

**`CommandAction` is Codex parsing its own shell command for the client:**

```ts
type CommandAction =
  | { type: "read",      command, name, path }
  | { type: "listFiles", command, path }
  | { type: "search",    command, query, path }
  | { type: "unknown",   command }
```

This is exactly the "kind column" Fleet wants, computed by the harness. Claude gives Fleet
structured tools (`Read`, `Grep`, `Glob`) directly; Codex funnels almost everything through
shell and hands back this parse instead. **Fleet's tool-kind classifier must consume
`commandActions` for Codex and `tool_name` for Claude, and must degrade to the raw command line
when the action is `unknown`.**

`UserInput` is the send vocabulary:

```ts
type UserInput =
  | { type: "text", text, text_elements: TextElement[] }
  | { type: "image", detail?, url } | { type: "localImage", detail?, path }
  | { type: "audio", url }          | { type: "localAudio", path }
  | { type: "skill", name, path }   | { type: "mention", name, path }
```

`skill` and `mention` are **structured** — Fleet's composer should emit them as typed parts
rather than interpolating `@path` into the text. Claude takes a plain string message and does
its own `@`/`/` parsing.

## 6. Controls

```ts
type ThreadSettings = {
  cwd, approvalPolicy: AskForApproval, approvalsReviewer, sandboxPolicy: SandboxPolicy,
  activePermissionProfile, model: string, modelProvider, serviceTier,
  effort: ReasoningEffort | null, summary: ReasoningSummary | null,
  collaborationMode, multiAgentMode /* deprecated */, personality }

type AskForApproval =
  | "untrusted" | "on-request" | "never"
  | { granular: { sandbox_approval, rules, skill_approval, request_permissions, mcp_elicitations } }

type SandboxMode = "read-only" | "workspace-write" | "danger-full-access"
type PermissionGrantScope = "turn" | "session"
```

Three independent axes Fleet must not collapse into one "mode": **approval policy**,
**sandbox policy**, and **permission profile**. Claude has one `--permission-mode` plus a rule
list. This is the single largest modelling divergence between the two harnesses.

Model + effort:

```ts
type Model = { id, model, displayName, description, modelSpecialty, hidden,
  supportedReasoningEfforts: ReasoningEffortOption[],   // per-model legal set
  defaultReasoningEffort: ReasoningEffort,
  inputModalities, supportsPersonality, serviceTiers, defaultServiceTier, isDefault, ... }
type ReasoningEffortOption = { reasoningEffort: ReasoningEffort, description: string }
```

**The legal effort set is per model and comes from `model/list`.** Fleet must not hardcode a
ladder; it renders `supportedReasoningEfforts` with their `description` strings.

Where settings may change:

| Scope | Mechanism |
| --- | --- |
| Thread creation | `thread/start` params |
| Subsequent turns | `thread/settings/update` — explicitly "for subsequent turns" |
| One turn only | `turn/start` params (`model`, `effort`, `sandboxPolicy`, `approvalPolicy`, `personality`, …) |

Nothing changes the **running** turn. A model or effort switch mid-turn takes effect on the next
turn; Fleet's UI must say so rather than implying it applies now.

## 7. Steering

`turn/steer` takes `expectedTurnId` and the same `UserInput[]` as `turn/start`. If the turn is
not steerable, the call fails with `activeTurnNotSteerable { turnKind: NonSteerableTurnKind }`.
So Fleet's queue/steer decision is **answerable by the harness**, not guessed: try to steer, and
on that specific error fall back to queueing until the turn settles. Claude has no steer
method — a second `user` line during a running turn is coalesced by the CLI instead.

## 8. Divergences Fleet must carry, not flatten

| Axis | Claude Code | Codex |
| --- | --- | --- |
| Approval option set | derived from `permission_suggestions` | `availableDecisions`, ordered, authoritative |
| Approval scope | `once` / session rules with `destination` | `accept` / `acceptForSession`; `PermissionGrantScope = turn \| session` |
| Extra approval kinds | — | network policy amendment, execpolicy amendment, permission profile widening |
| Question keying | by exact question **text** | by question **id** |
| Question secrecy | — | `isSecret` (must be masked, never persisted) |
| Question blocking | always blocking | `isBlocking: false` exists |
| Reasoning channels | one (`thinking_delta`) | two (`summary` per index, `content` raw) |
| Tool identity | structured tool name | shell command + `commandActions` parse |
| Turn diff | derive from edit items | `turn/diff/updated`, free |
| Attention | derive from open gates | `ThreadStatus.activeFlags` |
| Steering | implicit coalescing of a second `user` line | explicit `turn/steer` + `expectedTurnId` |
| History | Fleet must store it | `thread/items/list` / `thread/turns/list`, paginated, harness-side |
| Error taxonomy | `subtype` / `is_error` / `terminal_reason` strings | typed `CodexErrorInfo` union + `willRetry` |
| Settings axes | one permission mode + rules | approval policy × sandbox policy × permission profile |
| Effort ladder | not exposed on `system/init` | per-model `supportedReasoningEfforts` from `model/list` |
| Cancelled gate | `control_cancel_request` | `serverRequest/resolved` |
| Compaction | `system/compact_boundary` | `thread/compacted` + `contextCompaction` item |
| Sub-agents | `Task` tool + `task_*` system events | `subAgentActivity` item + `collabAgentToolCall` |

## 9. Live validation (codex-cli 0.147.0)

Driven against a real `codex app-server` over stdio: 40 frames, one turn, one shell tool call.
The capture and its driver script belong under `docs/research/fixtures/agents/codex/` and are a
follow-up (`NATIVE-AGENTS.md` §13, phase 1).

### 9.1 There IS an `initialize` handshake

```
→ {"jsonrpc":"2.0","id":1,"method":"initialize",
   "params":{"clientInfo":{"name":"fleet-probe","title":"Fleet","version":"0.0.1"}}}
← {"result":{"userAgent":"fleet-probe/0.147.0 (Linux Unknown; x86_64) xterm-256color (fleet-probe; 0.0.1)",
             "codexHome":"/home/df/.codex","platformFamily":"unix","platformOs":"linux"}}
```

`codexHome` comes back resolved — Fleet gets the config root without guessing it. The
`clientInfo` Fleet sends is echoed into the user agent, so Fleet identifies itself here.

### 9.2 The observed frame sequence for one ordinary turn

```
→ initialize                      ← result, remoteControl/status/changed
→ thread/start                    ← mcpServer/startupStatus/updated, result{thread}, thread/started
→ turn/start                      ← result{turn}, thread/status/changed{active,activeFlags:[]}, turn/started
                                  ← item/started{userMessage}      item/completed{userMessage}
                                  ← item/started{agentMessage}     ×10 item/agentMessage/delta   item/completed
                                  ← item/started{commandExecution} item/completed{commandExecution}
                                  ← thread/tokenUsage/updated,     account/rateLimits/updated
                                  ← item/started{agentMessage}     ×4 item/agentMessage/delta    item/completed
                                  ← thread/tokenUsage/updated,     account/rateLimits/updated
                                  ← thread/status/changed{idle},   turn/completed
```

Confirmations that matter:

- **The user's own message is echoed back as a `userMessage` item.** Fleet must reconcile its
  optimistic local echo against this item rather than appending a duplicate. `clientId` on the
  item is the `clientUserMessageId` Fleet sent — that is the reconciliation key, and Fleet
  should always send one.
- `thread/status/changed` fires `active` **before** `turn/started` and `idle` **before**
  `turn/completed`. Status is a thread-level hint; `turn/completed` remains the authority.
- `item/started` for a `commandExecution` already carries `status:"inProgress"` and the full
  `commandActions` parse; `item/completed` carries `status:"completed"` and `exitCode`. There is
  no separate "tool result" frame — the completed item **is** the result.
- Token usage and rate limits are pushed **per model step**, not once per turn.

### 9.3 `commandActions` in the wild — this is the kind column, computed by the harness

Codex ran a shell command; the raw `command` field is:

```
/usr/bin/bash -lc "sed -n '1,120p' note.txt"
```

which is unrenderable. But the same item carries:

```json
"commandActions":[{"type":"read","command":"sed -n '1,120p' note.txt",
                   "name":"note.txt","path":"/…/cwt/note.txt"}]
```

So a Codex `commandExecution` row renders as `read · note.txt` using `commandActions[0]`, and
falls back to the raw command line only when the action is `{type:"unknown"}`. **Fleet must
render from `commandActions`, never from `command`** — otherwise every Codex tool row shows a
`bash -lc` wrapper where Claude shows a clean `Read`.

Note `commandActions` is an **array**: a piped command decomposes into several actions. The row
summary uses the first meaningful action and the expanded body lists all of them.

### 9.4 Token usage, live

```json
{"threadId":"…","turnId":"…","tokenUsage":{
  "total":{"totalTokens":39846,"inputTokens":39705,"cachedInputTokens":30848,
           "cacheWriteInputTokens":0,"outputTokens":141,"reasoningOutputTokens":0},
  "last": {"totalTokens":19941,"inputTokens":19933,"cachedInputTokens":19584,
           "cacheWriteInputTokens":0,"outputTokens":8,"reasoningOutputTokens":0},
  "modelContextWindow":258400}}
```

`total` is cumulative for the thread, `last` is the most recent model step, and
`modelContextWindow` is the denominator. The context meter is `total.totalTokens /
modelContextWindow`. Claude's equivalent denominator is `modelUsage[model].contextWindow` on the
`result` frame — but Claude only publishes it at turn end, whereas Codex pushes it live. **The
Codex context meter can move during a turn; the Claude one cannot.** Fleet should let it, and
not fake a live meter for Claude.

### 9.5 TRAP: `turn/completed.turn.items` is a *summary*, not the item list

In the successful capture the turn produced four items (`userMessage`, `agentMessage`,
`commandExecution`, `agentMessage`). The completion frame reported:

```json
{"status":"completed","durationMs":13802,"itemsView":"summary","items":["agentMessage"]}
```

**One item.** And `turn/started` reports `items: 0`, `status: "inProgress"`.

`Turn.itemsView` (`TurnItemsView`) governs how much of `items` is populated. A client that
rebuilds the transcript from `turn/completed.turn.items` silently loses every tool call.

Fleet's rule: **the transcript is accumulated from `item/started` / `item/completed`
notifications and their deltas. `turn/completed` contributes only `status`, `error`,
`durationMs`, `startedAt`, `completedAt`.** Backfill and scroll-back come from
`thread/items/list`, never from the turn envelope.

This is the Codex analogue of the Claude rule "the `result` frame settles the turn but does not
carry the transcript".

### 9.6 A real failure path

Captured unintentionally when the model was at capacity:

```json
// notification
{"method":"error","params":{
  "error":{"message":"Selected model is at capacity. Please try a different model.",
           "codexErrorInfo":"serverOverloaded","additionalDetails":null},
  "willRetry":false,"threadId":"…","turnId":"…"}}

// then, immediately
{"method":"turn/completed","params":{"turn":{
  "status":"failed",
  "error":{"message":"…","codexErrorInfo":"serverOverloaded","additionalDetails":null},
  "durationMs":13658,"items":[]}}}
```

Ordering: `thread/status/changed{idle}` → `error` → `turn/completed{status:"failed"}`.

So an `error` notification is **not** itself terminal — `turn/completed` still arrives and is
still the authority, exactly as with Claude's `result`. And `willRetry: true` means the error is
informational: Fleet renders a retry notice and keeps the turn running. Only
`turn/completed.status` decides `completed | interrupted | failed`.

`codexErrorInfo: "serverOverloaded"` is one of the typed variants, so Fleet can say *"model at
capacity — try another model"* with an actionable model-switch affordance, rather than printing
a raw string.

### 9.7 Approval capture — not obtained

`approvalPolicy: "untrusted"` with `sandbox: "read-only"` did not produce an
`item/commandExecution/requestApproval` in the attempt above, because the turn failed upstream
(model at capacity) before any command ran. The approval shapes in §3 are taken from the
generated schema for this exact CLI version and are authoritative; the live round-trip is still
worth capturing before the adapter ships, as a fixture.
