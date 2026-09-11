<!-- Distilled from the Claude Agent SDK plus live captures from the installed CLI.
     The OpenCode half (§B) is HISTORICAL: Fleet dropped OpenCode in ADR 0014. -->

# Headless wire protocols: Claude Code 2.1.266

Authority over: the Claude Code stream-json surface Fleet's adapter is written against.
`NATIVE-AGENTS.md` §4.1 fixes the decisions; this file is the reference.
The Codex reference is [harness-codex-app-server.md](harness-codex-app-server.md).

Verified 2026-09-10 against `claude --version` -> `2.1.266 (Claude Code)`, from a live driven
session over stdio: handshake, one ordinary turn with a tool call, a real permission gate, and a
real `AskUserQuestion`.

§B (OpenCode 1.17.18) is retained **as a historical appendix**. Fleet no longer supports OpenCode
(ADR 0014); the section is kept because it is the record of what was verified, and deleting
verified research to tidy a document destroys the evidence the decisions were informed.

Names and discriminants are case-sensitive. The protocol evolves additively: ignore unknown
fields and variants, and never reinterpret them.

## Corrections applied on 2026-09-10 (2.1.263 -> 2.1.266)

The previous revision of this file was verified against 2.1.263 and had drifted. Each row below
was confirmed from a live capture, and each one changed a design decision in
`NATIVE-AGENTS.md`.

| Previous claim | 2.1.266 |
| --- | --- |
| Claude exposes no reasoning-effort control | `--effort <low\|medium\|high\|xhigh\|max>` is a real session flag. It is **not** echoed on `system/init` (`.effort` is absent), so the launcher must remember what it passed. |
| `--permission-prompt-tool stdio` is the way to receive permission asks | Still **required**. `--permission-prompts <host\|none>` was added alongside it, but with `host` and no prompt tool a gated tool produces **no** `control_request` — it produces `system/permission_denied` and the tool is silently refused. |
| The permission modes are the plan/acceptEdits/bypass set | Six: `acceptEdits`, `auto`, `bypassPermissions`, `manual`, `dontAsk`, `plan`. The mode passed is not necessarily the mode reported back — `manual` comes back as `permissionMode: "default"`, so init's value is advisory. |
| No capability negotiation exists | `system/init.capabilities` is real: `["interrupt_receipt_v1", "interrupt_cancel_queued_v1", "msg_lifecycle_v1"]`. Gate on this, never on a version-string compare. |
| Echo the permission suggestion to allow "for this session" | The live suggestion came back `destination: "localSettings"`. Echoing it verbatim writes a **persistent** rule into the user's `.claude/settings.local.json`. Rewrite `destination` to `"session"` for a session-scoped choice. |
| — | Three frame kinds were undocumented here: `system/status` (a spinner sub-label, never terminal), `system/thinking_tokens` (a live reasoning-token estimate, `{estimated_tokens, estimated_tokens_delta}`), and `rate_limit_event` (two utilisation windows — see below). |
| — | `can_use_tool` carries `display_name`, which is the label to show rather than `tool_name`. |

### `rate_limit_event`

Arrives unsolicited, mid-stream:

```json
{"type":"rate_limit_event","rate_limit_info":{
  "status":"allowed","resetsAt":1789029000,"rateLimitType":"five_hour",
  "overageStatus":"rejected","overageDisabledReason":"org_level_disabled","isUsingOverage":false,
  "unifiedWindows":{"five_hour":{"utilization":0.67,"resetsAt":1789029000},
                    "seven_day":{"utilization":0.18,"resetsAt":1789596000}}}}
```

Two windows, each a utilisation fraction and an epoch-seconds reset. **A window whose `status` is
`"rejected"` with no allowed overage parks the turn inside the CLI: no further frames arrive and
no `result` ever lands.** That is the one case where waiting for the completion authority hangs
forever, and `NATIVE-AGENTS.md` §4.1.2 gives it a first-class `Waiting{UsageLimit}` state rather
than a warning row.

### `system/permission_denied`

```json
{"type":"system","subtype":"permission_denied","tool_name":"Bash",
 "tool_use_id":"toolu_01QKUC…","decision_reason_type":"other",
 "decision_reason":"This command requires approval",
 "message":"This command requires approval","uuid":"…","session_id":"…"}
```

This is the frame that says "this was refused without asking you". It **must** render as a denied
tool row; dropping it makes a refusal look like a hang.

## A. Claude Code stream-json over stdio

### Transport and launch

Both directions are UTF-8 NDJSON: one complete JSON object per line, no array, Content-Length, or SSE framing. Keep stdin open for multiple turns and control responses. stdout EOF/process exit is process lifetime, not a turn boundary.

Direct installed-CLI launch:

```text
/Users/me/.local/bin/claude -p \
  --output-format stream-json \
  --input-format stream-json \
  --verbose \
  --include-partial-messages \
  --permission-prompt-tool stdio
```

`-p` is the standalone CLI's documented print-mode gate. The Agent SDK does not literally add `-p`; with piped stdio its unconditional base argv is exactly:

```ts
["--output-format", "stream-json", "--verbose", "--input-format", "stream-json"]
```

For a Rust daemon invoking the standalone binary, pass `-p` explicitly. The two stream formats and `--verbose` are required. `--permission-prompt-tool stdio` is required to receive permission asks as `can_use_tool`; the SDK adds it when `canUseTool` is set. `--include-partial-messages` is optional but required for token deltas. `--replay-user-messages` is optional acknowledgement; the SDK never adds it itself.

#### Exact SDK option → argv construction

Order is the base argv, then these in source order. JSON is compact. Final `extraArgs` become `--key` for null, otherwise `--key value` (a value beginning `-` uses `--key=value`).

| SDK option | argv |
|---|---|
| `thinking:{type:"enabled",budgetTokens:N}` | `--max-thinking-tokens N`; absent budget → `--thinking adaptive` |
| `thinking.type:"adaptive"|"disabled"` | `--thinking adaptive|disabled` |
| enabled/adaptive `thinking.display` | `--thinking-display summarized|omitted` |
| `effort` | `--effort VALUE` |
| `maxTurns` | `--max-turns N` |
| `maxBudgetUsd` | `--max-budget-usd N` |
| `taskBudget.total` | `--task-budget N` |
| `model` | `--model MODEL` |
| `agent` | `--agent NAME` |
| nonempty `betas` | `--betas comma,separated` |
| output JSON schema | `--json-schema COMPACT_JSON` |
| `debugFile`; else `debug` | `--debug-file PATH`; else `--debug` |
| `canUseTool` | `--permission-prompt-tool stdio` |
| `permissionPromptToolName` | `--permission-prompt-tool NAME` (SDK rejects using both) |
| `permissionPrompts` | `--permission-prompts host|none` |
| `continue` | `--continue` |
| `resume` | one token `--resume=SESSION_ID` |
| `allowedTools` (+ synthesized skills) | `--allowedTools comma,separated` (capital `T`) |
| `disallowedTools` | `--disallowedTools comma,separated` |
| `tools:[]`; list; preset | `--tools ""`; comma list; `--tools default` |
| nonempty `mcpServers` | `--mcp-config '{"mcpServers":{...}}'` |
| defined `settingSources` | one token `--setting-sources=user,project,local`; empty suffix for `[]` |
| `strictMcpConfig` | `--strict-mcp-config` |
| `permissionMode` | `--permission-mode default|acceptEdits|bypassPermissions|plan|dontAsk|auto` |
| `allowDangerouslySkipPermissions` | `--allow-dangerously-skip-permissions` |
| `fallbackModel` | `--fallback-model VALUE` |
| `includeHookEvents` | `--include-hook-events` |
| `includePartialMessages` | `--include-partial-messages` |
| each `additionalDirectories` | repeated `--add-dir PATH` |
| `pluginDelivery:"initialize"` | `--await-initialize` |
| argv-delivered local plugins | repeated `--plugin-dir PATH` or `--plugin-dir-no-mcp PATH` |
| `forkSession` | `--fork-session` |
| `resumeSessionAt` | one token `--resume-session-at=UUID` |
| defined `resumeDropsTurn` | one token `--resume-drops-turn=UUID` |
| `sessionId` | one token `--session-id=UUID` |
| `persistSession:false` | `--no-session-persistence` |
| `managedSettings` | `--managed-settings VALUE` |
| `settings` | final flag map: `--settings VALUE` |

`systemPrompt`, append prompt, agents, title, filtered skills, progress/suggestion options, and SDK MCP names go in `initialize`, not normally argv. With initialize plugin delivery, plugins do too. `sdk.mjs` currently sends additive initialize keys `appendSubagentSystemPrompt` and `webSearchIsolationExemptMcpServers` that are absent from `SDKControlInitializeRequest`; tolerate them.

### stdin user message

```ts
type SDKUserMessage = {
  type:"user";
  message:MessageParam;                  // role:"user", content:string|blocks[]
  parent_tool_use_id:string|null;
  isSynthetic?:boolean;
  tool_use_result?:unknown;
  priority?:"now"|"next"|"later";
  origin?:SDKMessageOrigin;
  shouldQuery?:boolean;                  // false: append only, fold into later query
  timestamp?:string;
  uuid?:UUID;
  session_id?:string;
  subagent_type?:string;
  task_description?:string;
};
```

Useful full prompt:

```json
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Reply with pong"}]},"parent_tool_use_id":null,"isSynthetic":false,"priority":"now","origin":{"kind":"human"},"shouldQuery":true,"timestamp":"2026-09-07T12:00:00.000Z","uuid":"11111111-1111-4111-8111-111111111111","session_id":""}
```

Accepted minimal prompt (used by fixture):

```json
{"type":"user","message":{"role":"user","content":"Reply with exactly the word: pong"}}
```

Image block: `{"type":"image","source":{"type":"base64","media_type":"image/png","data":"..."}}`. Set a client UUID when possible; supported producers echo it as `user_message_uuid` and `user_message_uuids`. Queued messages may coalesce; singular UUID is the last member and the plural field is the ordered consumed set.

### Control envelopes

```ts
type SDKControlRequest = {
  type:"control_request"; request_id:string;
  request:{subtype:string; /* fields */};
};
type SDKControlResponse = {
  type:"control_response";
  response:
    | {subtype:"success";request_id:string;response?:Record<string,unknown>;
       pending_permission_requests?:SDKControlRequest[];
       pending_user_dialog_requests?:SDKControlRequest[]}
    | {subtype:"error";request_id:string;error:string;
       pending_permission_requests?:SDKControlRequest[];
       pending_user_dialog_requests?:SDKControlRequest[]};
};
type SDKControlCancelRequest = {type:"control_cancel_request";request_id:string};
```

Every request expects one same-ID response except `control_cancel_request`, which may flow either way and has no response. The sender stops waiting and ignores a late response.

#### Complete public/core subtype catalog

| Direction | request payload after `subtype` | success `response` |
|---|---|---|
| client→CLI | `initialize`: `hooks?`, `sdkMcpServers?:string[]`, `sdkMcpServerConfigs?:{[name]:{timeout?:number}}`, `jsonSchema?`, `systemPrompt?:string[]`, `appendSystemPrompt?`, `systemPromptSnapshot?`, `planModeInstructions?`, `toolAliases?`, `excludeDynamicSections?`, `agents?`, `title?`, `skills?:string[]`, `promptSuggestions?`, `agentProgressSummaries?`, `forwardSubagentText?`, `supportedDialogKinds?:string[]`, `perTaskStopAffordance?`, `plugins?` | initialization object below; outer success may include pending-request snapshots |
| client→CLI | `interrupt {cancel_queued?:boolean}` | `{still_queued:string[],cancelled?:string[]}` when capability advertised; older CLI `{}` |
| client→CLI | `set_permission_mode {mode}` | `{}` |
| client→CLI | `set_model {model?:string|null}` | `{}` |
| client→CLI | `set_max_thinking_tokens {max_thinking_tokens?:number|null,thinking_display?:"summarized"|"omitted"|null}` | `{}` |
| client→CLI | `rename_session {title:string}`; `set_color {color:string}` | `{}` |
| client→CLI | `apply_flag_settings {settings:object}` | `{}` |
| client→CLI | `update_settings {source:"localSettings",settings:object}` | `{}`; writer currently allowlists string `outputStyle` |
| client→CLI | `mcp_status {}` | `{mcpServers:McpServerStatus[]}` |
| client→CLI | `get_context_usage {detail?:"summary"|"full"}` | exported context usage object: categories/totals/window/grid/model/memory/MCP/tools/agents/skills/message breakdown/apiUsage |
| client→CLI | `get_session_cost {}` | internal formatted-cost object; public Query exports no response type |
| client→CLI | `get_settings {}` | opaque merged/raw settings object; no response type is exported |
| client→CLI | `list_models {}` | model catalog object; Query normally uses `initialize.models` |
| client→CLI | `get_usage {skip_behaviors?:boolean}` | exported experimental usage object: session/model totals, subscription, rate windows, behavior attribution |
| client→CLI | `get_binary_version {}` | internal version object |
| client→CLI | `mcp_call {tool,arguments?,expires_at?,timeout_ms?,input_files?:[{name,lane_path}],output_files?:[{name,lane_path,if_match?}]}` | additive MCP result/staging object |
| client→CLI | `file_suggestions {query}` | internal suggestions object |
| client→CLI | `rewind_files {user_message_id,dry_run?}` | `{canRewind,error?,filesChanged?,insertions?,deletions?,skippedLinks?}` |
| client→CLI | `cancel_async_message {message_uuid}` | `{cancelled:boolean}` |
| client→CLI | `read_file {path,max_bytes?,encoding?:"utf-8"|"base64"}` | `{contents,absPath,truncated?,encoding?:"base64"}` |
| client→CLI | `seed_read_state {path,mtime}` | `{}` |
| client→CLI | `mcp_set_servers {servers:{[name]:config}}` | `{added:string[],removed:string[],errors:Record<string,string>}` plus additive fields |
| client→CLI | `register_repo_root {directory,reload_claude_md?,reload_plugins?,reload_skills?}` | additive registration/reload result (not named in d.ts) |
| client→CLI | `reload_plugins {}` | `{commands,agents,plugins:[{name,path,source?,version?}],mcpServers,error_count}` |
| client→CLI | `reload_skills {}` | `{skills:SlashCommand[]}` |
| client→CLI | `reload_output_styles {}` | `{available_output_styles:string[]}` |
| client→CLI | `mcp_reconnect {serverName}`; `mcp_toggle {serverName,enabled}`; `stop_task {task_id}` | `{}`; stopped task later emits notification |
| client→CLI | `background_tasks {tool_use_id?:string}` | `{backgrounded:boolean}` (SDK defaults absent key to true) |
| CLI→client | `can_use_tool` | permission result below |
| CLI→client | `hook_callback {callback_id,input:HookInput,tool_use_id?}` | HookJSONOutput, returned verbatim |
| both | `mcp_message {server_name,message:JSONRPCMessage}` | CLI→client: `{mcp_response:JSONRPCMessage}`; client→CLI acknowledged `{}` |
| CLI→client | `elicitation {mcp_server_name,message,mode?,url?,elicitation_id?,requested_schema?,title?,display_name?,description?}` | e.g. `{action:"accept",content?:object}` or `{action:"decline"}`; SDK defaults decline without handler |
| CLI→client | `request_user_dialog {dialog_kind,payload,tool_use_id?}` | kind-specific result; only answer kinds declared in `supportedDialogKinds` |

Runtime/type mismatch: `sdk.mjs` also sends `set_mcp_permission_mode_override {serverName,mode:"default"|"auto"|null}` and expects `{warning?:string}`, although absent from `SDKControlRequestInner`. The bundle contains private remote-control/UI subtypes (`set_cwd`, OAuth, side-question, feedback, etc.) not exposed by headless `Query`; a native adapter need not originate them. Error unknown incoming requests rather than guessing.

Initialize example (undefined keys are omitted in reality):

```json
{"type":"control_request","request_id":"init-1","request":{"subtype":"initialize","hooks":{"PreToolUse":[{"matcher":"Bash","hookCallbackIds":["hook_0"],"timeout":30}]},"sdkMcpServers":["local"],"sdkMcpServerConfigs":{"local":{"timeout":120000}},"jsonSchema":{"type":"object"},"systemPrompt":["system"],"appendSystemPrompt":"append","systemPromptSnapshot":true,"planModeInstructions":"workflow","toolAliases":{"Bash":"mcp__workspace__bash"},"excludeDynamicSections":false,"agents":{"reviewer":{"description":"Review","prompt":"Review carefully"}},"title":"Title","skills":["pdf"],"promptSuggestions":true,"agentProgressSummaries":true,"forwardSubagentText":true,"supportedDialogKinds":["refusal_fallback_prompt"],"perTaskStopAffordance":true,"plugins":[{"type":"local","path":"/abs/plugin"}]}}
```

```ts
type SDKControlInitializeResponse = {
  commands:SlashCommand[]; agents:AgentInfo[];
  output_style:string; available_output_styles:string[];
  models:ModelInfo[]; account:AccountInfo;
  hooks_applied?:boolean; plugins_applied?:boolean;
  fast_mode_state?:FastModeState;
  fast_mode_disabled_reason?:FastModeDisabledReason;
};
```

Setter example:

```json
{"type":"control_request","request_id":"m-1","request":{"subtype":"set_model","model":"claude-sonnet-5"}}
{"type":"control_response","response":{"subtype":"success","request_id":"m-1","response":{}}}
```

### Permissions, AskUserQuestion, ExitPlanMode

```ts
type CanUseToolRequest = {
  subtype:"can_use_tool"; tool_name:string; input:Record<string,unknown>;
  permission_suggestions?:PermissionUpdate[]; blocked_path?:string;
  decision_reason?:string;
  decision_reason_type?:"rule"|"mode"|"subcommandResults"|"permissionPromptTool"|"hook"|"asyncAgent"|"sandboxOverride"|"workingDir"|"safetyCheck"|"classifier"|"other";
  classifier_approvable?:boolean; suppress_always_allow_rule?:boolean;
  default_to_no?:boolean;
  matched_ask_rule?:{source:string;tool_name:string;rule_content?:string};
  title?:string; display_name?:string; tool_use_id:string; agent_id?:string;
  description?:string; requires_user_interaction?:boolean;
};
```

All-fields request example:

```json
{"type":"control_request","request_id":"perm-1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"echo fixture"},"permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"echo *"}],"behavior":"allow","destination":"session"}],"blocked_path":"/outside","decision_reason":"Command needs approval","decision_reason_type":"mode","classifier_approvable":false,"suppress_always_allow_rule":false,"default_to_no":true,"matched_ask_rule":{"source":"userSettings","tool_name":"Bash","rule_content":"Bash(*)"},"title":"Claude wants to run a command","display_name":"Run command","tool_use_id":"toolu_123","agent_id":"agent_123","description":"echo fixture","requires_user_interaction":false}}
```

Allow and deny:

```json
{"type":"control_response","response":{"subtype":"success","request_id":"perm-1","response":{"behavior":"allow","updatedInput":{"command":"echo fixture"},"updatedPermissions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"echo *"}],"behavior":"allow","destination":"session"}],"toolUseID":"toolu_123","decisionClassification":"user_permanent"}}}
{"type":"control_response","response":{"subtype":"success","request_id":"perm-1","response":{"behavior":"deny","message":"User declined tool execution.","interrupt":false,"toolUseID":"toolu_123","decisionClassification":"user_reject"}}}
```

`PermissionUpdate`: `addRules|replaceRules|removeRules {rules:[{toolName,ruleContent?}],behavior:"allow"|"deny"|"ask",destination}`, `setMode {mode,destination}`, or `addDirectories|removeDirectories {directories,destination}`. Destinations: `userSettings|projectSettings|localSettings|session|cliArg`. `decision_reason` may contain ANSI; sanitize. Honor `suppress_always_allow_rule`, `default_to_no`, and `requires_user_interaction`.

`AskUserQuestion` is `can_use_tool` with that tool name. Input: 1–4 `questions`, each `{question,header,options:[{label,description,preview?}]` (2–4), `multiSelect:boolean}`. Preserve questions and key answers by exact full question text:

```json
{"type":"control_response","response":{"subtype":"success","request_id":"q-1","response":{"behavior":"allow","updatedInput":{"questions":[{"question":"Which database?","header":"Database","options":[{"label":"SQLite","description":"Local"},{"label":"Postgres","description":"Server"}],"multiSelect":false}],"answers":{"Which database?":"SQLite"}},"toolUseID":"toolu_q"}}}
```

Multi-select values may be arrays of labels; custom text is a string. Cancellation is deny.

`ExitPlanMode` is also `can_use_tool`. Declared input has only deprecated `allowedPrompts?` plus unknown keys; the plan markdown arrives as `input.plan`. The assistant `tool_use` block is another place to capture it. To approve execution, allow with unchanged input. To make the proposal a terminal handoff, deny with a message saying the client captured the plan and Claude must wait; t3code does this. There is no separate plan-response frame.

Without stdio permission prompting, asks do not become controls. `--permission-prompts none` immediately denies asks. `system/permission_denied` is advisory *as a tally* — `result.permission_denials` is the authoritative list — but the frame must still render as a denied tool row, or a refusal looks like a hang.

### stdout catalog

All normal messages carry `uuid` and `session_id` unless stated. Raw controls and `{"type":"keep_alive"}` are transport frames; the SDK consumes controls instead of yielding them as `SDKMessage`.

| top-level `type` | exact payload/semantics |
|---|---|
| `assistant` | `message:BetaMessage`, `parent_tool_use_id`, `error?` (`authentication_failed|oauth_org_not_allowed|account_on_hold|billing_error|rate_limit|overloaded|invalid_request|model_not_found|server_error|unknown|max_output_tokens`), `request_id?`, first-frame `user_message_uuid?`, `user_message_uuids?`, `resumed_from_incomplete_thinking?:true`, `supersedes?:UUID[]`, `aborted?:true`, `subagent_type?`, `task_description?`, `timestamp?`, `context_usage?`. Multiple frames can share `message.id`, one completed content block each. |
| `user` | user envelope emitted chiefly for `tool_result`; `tool_use_result` is structured full output. Replay acknowledgement has `isReplay:true`. |
| `stream_event` | `event:BetaRawMessageStreamEvent,parent_tool_use_id,ttft_ms?,user_message_uuid?,user_message_uuids?`. |
| `result` | exactly one per turn; definitions below. |
| `tool_progress` | `tool_use_id,tool_name,parent_tool_use_id,elapsed_time_seconds,task_id?,heartbeat?,subagent_type?,subagent_retry?:{agent_id,attempt,max_retries,retry_delay_ms,error_status,error_category}`. |
| `tool_use_summary` | `summary,preceding_tool_use_ids`. |
| `auth_status` | `isAuthenticating:boolean,output:string[],error?:string`. |
| `rate_limit_event` | `rate_limit_info:{status:"allowed"|"allowed_warning"|"rejected",resetsAt?,rateLimitType?,utilization?,overageStatus?,overageResetsAt?,overageDisabledReason?,isUsingOverage?,overageInUse?,surpassedThreshold?,errorCode?,canUserPurchaseCredits?,hasChargeableSavedPaymentMethod?}`; fixture also shows additive `unifiedWindows`. |
| `prompt_suggestion` | `suggestion`; may follow result. |
| `conversation_reset` | `new_conversation_id`. |
| `active_goal` | `value:{condition,iterations,set_at,tokens_at_start,last_reason?}|null`. It is present in the raw `StdoutMessage` union but omitted from the narrower public `SDKMessage` union; a direct Rust decoder must still accept it. |

`stream_event.event.type`: `message_start {message}`, `content_block_start {index,content_block}`, `content_block_delta {index,delta}`, `content_block_stop {index}`, `message_delta {delta,usage,...}`, `message_stop`. Delta kinds: `text_delta {text}`, `input_json_delta {partial_json}`, `thinking_delta {thinking}`, `signature_delta {signature}`, `citations_delta {citation}`. Buffer input JSON fragments through block stop.

System subtypes:

| subtype | fields after subtype |
|---|---|
| `init` | `agents?,apiKeySource,betas?,claude_code_version,cwd,tools,mcp_servers:[{name,status}],model,permissionMode,slash_commands,terminal_slash_commands?,output_style,skills,plugins:[{name,path,version?}],fast_mode_state?,fast_mode_disabled_reason?,effort?,capabilities?`. The live fixture additionally has `analytics_disabled`, `product_feedback_disabled`, `memory_paths:{auto}`, `messaging_socket_path`, and `plugins[].source`. Emitted each turn. |
| `status` | `status:"compacting"|"requesting"|null,permissionMode?,compact_result?:"success"|"failed",compact_error?`. |
| `compact_boundary` | `compact_metadata:{trigger:"manual"|"auto",pre_tokens,post_tokens?,duration_ms?,preserved_segment?:{head_uuid,anchor_uuid,tail_uuid},preserved_messages?:{anchor_uuid,uuids}}`. |
| `api_retry` | `attempt,max_retries,retry_delay_ms,error_status:number|null,error,no_response?:{waited_ms,retry_wait_ms}`. |
| `control_request_progress` | `request_id,status:"started"|"api_retry",attempt?,max_retries?,retry_delay_ms?,error_status?`. |
| `session_state_changed` | `state:"idle"|"running"|"requires_action"`; whole-session level. |
| `permission_denied` | `tool_name,tool_use_id,agent_id?,decision_reason_type?,decision_reason?,message`. |
| `hook_started` | `hook_id,hook_name,hook_event`. |
| `hook_progress` | `hook_id,hook_name,hook_event,stdout,stderr,output`. |
| `hook_response` | `hook_id,hook_name,hook_event,output,stdout,stderr,exit_code?,outcome:"success"|"error"|"cancelled"`. |
| `task_started` | `task_id,tool_use_id?,description,subagent_type?,is_backgrounded?,spawn_depth?,task_type?,workflow_name?,prompt?,skip_transcript?,ambient?`. |
| `task_updated` | `task_id,patch:{status?:"pending"|"running"|"completed"|"failed"|"killed"|"paused",description?,end_time?,total_paused_ms?,error?,is_backgrounded?}`. |
| `task_progress` | `task_id,tool_use_id?,description,subagent_type?,usage:{total_tokens,tool_uses,duration_ms},last_tool_name?,summary?`; runtime may add workflow progress. |
| `task_notification` | `task_id,tool_use_id?,status:"completed"|"failed"|"stopped",output_file,summary,usage?,resource_links?,skip_transcript?,ambient?`. |
| `background_tasks_changed` | `tasks:[{task_id,task_type,description,ambient?}]`; replace semantics; reset on process restart. |
| `thinking_tokens` | `estimated_tokens,estimated_tokens_delta,user_message_uuid?`; estimate only. |
| `commands_changed` | `commands:SlashCommand[]`; replace cached list. |
| `worker_shutting_down` | `reason`; live-tail hint, not replay-safe proof. |
| `elicitation_complete` | `mcp_server_name,elicitation_id`. |
| `files_persisted` | `files:[{filename,file_id}],failed:[{filename,error}],processed_at`. |
| `plugin_install` | `status:"started"|"installed"|"failed"|"completed",name?,error?`. |
| `informational` | `content,level:"info"|"notice"|"suggestion"|"warning",tool_use_id?,prevent_continuation?`. |
| `local_command_output` | `content`. |
| `memory_recall` | `mode:"select"|"synthesize",memories:[{path,scope:"personal"|"team"|"organization",content?}]`. |
| `mirror_error` | `error,key:{projectKey,sessionId,subpath?}`. |
| `model_refusal_fallback` | `trigger:"refusal",direction:"retry"|"revert"|"sticky",scope?,original_model,fallback_model,request_id,api_refusal_category?,api_refusal_explanation?,retracted_message_uuids?,refused_user_message_uuid?,content`. |
| `model_refusal_no_fallback` | `original_model,request_id,api_refusal_category?,api_refusal_explanation?,refused_user_message_uuid?,content`. |
| `notification` | `key,text,priority:"low"|"medium"|"high"|"immediate",color?,timeout_ms?`. |

Result core types:

```ts
type SDKPermissionDenial={tool_name:string;tool_use_id:string;tool_input:Record<string,unknown>};
type NonNullableUsage={[K in keyof BetaUsage]:NonNullable<BetaUsage[K]>};
type TerminalReason="blocking_limit"|"rapid_refill_breaker"|"prompt_too_long"|"image_error"|"model_error"|"api_error"|"malformed_tool_use_exhausted"|"aborted_streaming"|"aborted_tools"|"stop_hook_prevented"|"hook_stopped"|"tool_deferred"|"max_turns"|"background_requested"|"completed"|"budget_exhausted"|"structured_output_retry_exhausted"|"tool_deferred_unavailable"|"turn_setup_failed";
type ModelUsage={inputTokens:number;outputTokens:number;thinkingTokens?:number;
 cacheReadInputTokens:number;cacheCreationInputTokens:number;webSearchRequests:number;
 costUSD:number;contextWindow:number;maxOutputTokens:number;canonicalModel?:string;
 provider?:string;costBasis?:"list"|"managed"|"unknown"};
type SDKResultError={type:"result";
 subtype:"error_during_execution"|"error_max_turns"|"error_max_budget_usd"|"error_max_structured_output_retries";
 duration_ms:number;duration_api_ms:number;is_error:boolean;num_turns:number;
 stop_reason:string|null;total_cost_usd:number;usage:NonNullableUsage;
 modelUsage:Record<string,ModelUsage>;permission_denials:SDKPermissionDenial[];
 queued_turn_count?:number;errors:string[];user_message_uuid?:string;
 user_message_uuids?:string[];terminal_reason?:TerminalReason;origin?:SDKMessageOrigin;
 uuid:UUID;session_id:string};
type SDKResultSuccess={type:"result";subtype:"success";duration_ms:number;duration_api_ms:number;
 ttft_ms?:number;ttft_stream_ms?:number;time_to_request_ms?:number;
 user_message_uuid?:string;user_message_uuids?:string[];request_sent_wall_ms?:number;
 first_content_frame_ms?:number;first_stream_post_ms?:number;first_stream_post_ack_ms?:number;
 first_stream_post_wall_ms?:number;time_to_request_from_spawn_ms?:number;
 warm_spare_claimed?:boolean;time_origin_ms?:number;is_error:boolean;api_error_status?:number|null;
 num_turns:number;result:string;stop_reason:string|null;total_cost_usd:number;
 usage:NonNullableUsage;modelUsage:Record<string,ModelUsage>;
 permission_denials:SDKPermissionDenial[];queued_turn_count?:number;structured_output?:unknown;
 deferred_tool_use?:{id:string;name:string;input:Record<string,unknown>};
 terminal_reason?:TerminalReason;origin?:SDKMessageOrigin;uuid:UUID;session_id:string};
```

`NonNullableUsage` is the SDK's exact declaration: it makes every field of the imported Anthropic `BetaUsage` non-null. The installed fixture emitted `input_tokens,cache_creation_input_tokens,cache_read_input_tokens,output_tokens,output_tokens_details,server_tool_use,service_tier,cache_creation,inference_geo,iterations,speed`; decode it additively. The live result also carried additive `subagent_stats`, which is absent from `SDKResultSuccess` in `sdk.d.ts`.

Both variants may also carry `fast_mode_state` and `fast_mode_disabled_reason`. A success subtype can have `is_error:true`; inspect both. `usage` is main-loop/per-turn. `modelUsage` and cost are cumulative across the current query process and include pipeline subcalls; use the latest value, never sum results. Permission denials list is authoritative.

### Completion, interrupt, resume/fork

- `result` is the authoritative per-turn terminal. Exactly one follows that turn's assistant/user/stream frames. System/task/rate/prompt-suggestion frames may follow and the stream stays open.
- `queued_turn_count>0` means queued user sends remain, possibly coalescing. `session_state_changed idle` is stronger whole-session idle after held background work, but does not replace each result.
- Interrupt: `{"type":"control_request","request_id":"i-1","request":{"subtype":"interrupt","cancel_queued":true}}`. Capabilities `interrupt_receipt_v1` and `interrupt_cancel_queued_v1` are advertised here. Clean receipt precedes interrupted result; a crash race may reverse them. False/absent preserves queued UUID sends; true cancels those still owned by the process. Arrays do not cover all UUID-less/system/subagent work.
- The interrupted turn still emits result. Use terminal reason (`aborted_streaming`/`aborted_tools` when applicable) plus outstanding interrupt state. Do not wait for EOF.
- Fresh chosen ID: `--session-id=NEW_UUID`. Resume: `--resume=OLD_ID`; `--continue` selects latest for cwd.
- Fork: `--resume=OLD_ID --fork-session`; optionally `--session-id=NEW_UUID`. Without fork, resume continues the old ID.
- Truncating fork: `--resume-session-at=CHAIN_UUID`; `--resume-drops-turn=PROMPT_UUID` validates the dropped suffix. Deterministic refusal is an `error_during_execution` message beginning `Resume rejected by --resume-drops-turn:`; recover with plain resume, not the same retry.
- `--replay-user-messages` is acknowledgement, not resume. Resumed cost/accounting starts fresh for the new process.

### Claude fixtures

- `fixtures/claude-basic.ndjson`: 16 raw lines, successful `pong`; includes hook lifecycle, init/status, partial sequence, complete assistant, rate event, result. SessionStart hooks reported sandbox-denied `~/.claude/session-env` writes but the turn succeeded.
- `fixtures/claude-permission-{stdin,stdout}.ndjson`: one allowed attempt. Local rules auto-allowed `Bash(echo fixture)`, so no `can_use_tool` and no control response occurred. Tool result and final result succeeded. The capture then timed out at 120 s because stdin stayed open waiting for a request. The invocation was configured to route asks through stdio, but the flag does not force an already-allowed action to ask.

## B. OpenCode 1.17.18 HTTP + SSE — HISTORICAL

> **Fleet does not support OpenCode.** It was dropped in
> [ADR 0014](../decisions/0014-drop-opencode-add-codex.md) in favour of Codex. Everything below
> was verified against OpenCode 1.17.18 and is retained as the record of that verification. Do
> not implement against it.

### Base transport and scoping

Server command:

```text
/Users/me/.opencode/bin/opencode serve --hostname 127.0.0.1 --port 4597
```

The test sandbox required scratch-local `XDG_DATA_HOME`, `XDG_STATE_HOME`, and `XDG_CACHE_HOME` so OpenCode could write its log. Production should supervise the child and capture stdout/stderr. Without `OPENCODE_SERVER_PASSWORD` the server warns that it is unsecured. If a password is configured, the SDK uses HTTP Basic auth with username `opencode` and that password.

The v2 SDK's `createOpencodeClient({baseUrl,directory})` adds `directory` as a query parameter. Nearly every endpoint below also accepts optional `workspace`. Always send the exact canonical working directory; it selects project/config/storage context. A server can host multiple directories, so session ID alone is not the whole routing key.

JSON requests use `Content-Type: application/json`. The schemas below are for the legacy/current `/session`, `/event`, etc. surface used by the v2 SDK, not the separate `/api/session/...` durable API also present in this build's OpenAPI.

### Session and prompt endpoints

`Session`:

```ts
type Session={
 id:string; slug:string; projectID:string; workspaceID?:string;
 directory:string; path?:string; parentID?:string;
 summary?:{additions:number;deletions:number;files:number;diffs?:SnapshotFileDiff[]};
 cost?:number; tokens?:{input:number;output:number;reasoning:number;cache:{read:number;write:number}};
 share?:{url:string}; title:string; agent?:string;
 model?:{id:string;providerID:string;variant?:string}; version:string;
 metadata?:Record<string,unknown>;
 time:{created:number;updated:number;compacting?:number;archived?:number};
 permission?:PermissionRuleset;
 revert?:{messageID:string;partID?:string;snapshot?:string;diff?:string};
};
type PermissionRuleset={permission:string;pattern:string;action:"allow"|"deny"|"ask"}[];
```

| Operation | Method/path | body/query → success |
|---|---|---|
| Create | `POST /session?directory=...` | optional `{parentID?,title?,agent?,model?:{id,providerID,variant?},metadata?,permission?,workspaceID?}` → `200 Session`. Note create-model key is `id`, unlike prompt's `modelID`. |
| Get/resume | `GET /session/{sessionID}?directory=...` | → `200 Session`; `404 NotFoundError`. |
| List | `GET /session` | query `directory?,workspace?,scope?:"project",path?,roots?:boolean|"true"|"false",start?,search?,limit?` → `Session[]`. |
| Fork | `POST /session/{sessionID}/fork?directory=...` | optional `{messageID?}` → `200 Session` with new ID/parent. Omit message ID to fork current state. |
| Update | `PATCH /session/{sessionID}` | `{title?,metadata?,permission?,time?:{archived?}}` → `Session`. |
| Messages + parts | `GET /session/{sessionID}/message` | query `limit?:integer,before?:string` → `[{info:Message,parts:Part[]}]`. |
| One message | `GET /session/{sessionID}/message/{messageID}` | → `{info:Message,parts:Part[]}`. |
| Prompt, synchronous | `POST /session/{sessionID}/message` | prompt body below → `200 {info:AssistantMessage,parts:Part[]}` after the turn; HTTP error can still leave user message/events. |
| Prompt, asynchronous | `POST /session/{sessionID}/prompt_async` | same body → `204` acceptance only; observe SSE/status/messages for outcome. |
| Abort | `POST /session/{sessionID}/abort` | no body → `200 boolean`. |
| Status map | `GET /session/status?directory=...` | → `{[sessionID]:SessionStatus}`. Idle sessions may be absent; absence is idle after a valid map read. |
| Todo | `GET /session/{sessionID}/todo` | → `Todo[]`, each `{content,status,priority}`; documented values are pending/in_progress/completed/cancelled and high/medium/low, but fields are open strings. |

Prompt body:

```ts
type PromptBody={
 messageID?:string;
 model?:{providerID:string;modelID:string};
 agent?:string; noReply?:boolean; tools?:Record<string,boolean>;
 format?:{type:"text"}|{type:"json_schema";schema:Record<string,unknown>;retryCount?:number};
 system?:string; variant?:string;
 parts:(TextPartInput|FilePartInput|AgentPartInput|SubtaskPartInput)[];
};
```

Inputs:

```ts
type TextPartInput={id?:string;type:"text";text:string;synthetic?:boolean;ignored?:boolean;
 time?:{start:number;end?:number};metadata?:Record<string,unknown>};
type FilePartInput={id?:string;type:"file";mime:string;filename?:string;url:string;source?:FilePartSource};
type AgentPartInput={id?:string;type:"agent";name:string;source?:{value:string;start:number;end:number}};
type SubtaskPartInput={id?:string;type:"subtask";prompt:string;description:string;agent:string;
 model?:{providerID:string;modelID:string};command?:string};
```

Minimal asynchronous call:

```http
POST /session/ses_.../prompt_async?directory=%2Fabs%2Fwork
Content-Type: application/json

{"parts":[{"type":"text","text":"Reply with exactly the word: pong"}]}
```

Messages:

```ts
type UserMessage={id:string;sessionID:string;role:"user";time:{created:number};format?:OutputFormat;
 summary?:{title?:string;body?:string;diffs:SnapshotFileDiff[]};agent:string;
 model:{providerID:string;modelID:string;variant?:string};system?:string;tools?:Record<string,boolean>};
type AssistantMessage={id:string;sessionID:string;role:"assistant";
 time:{created:number;completed?:number};error?:MessageError;parentID:string;
 modelID:string;providerID:string;mode:string;agent:string;path:{cwd:string;root:string};
 summary?:boolean;cost:number;tokens:{total?:number;input:number;output:number;reasoning:number;
 cache:{read:number;write:number}};structured?:unknown;variant?:string;finish?:string};
type Message=UserMessage|AssistantMessage;
```

`MessageError` variants: `ProviderAuthError {providerID,message}`, `UnknownError {message,ref?}`, `MessageOutputLengthError`, `MessageAbortedError {message}`, `StructuredOutputError {message,retries}`, `ContextOverflowError {message,responseBody?}`, `ContentFilterError {message}`, `APIError {message,statusCode?,isRetryable,responseHeaders?,responseBody?,metadata?}`.

### Permissions and questions

Pending-state recovery endpoints:

- `GET /permission?directory=...` → `PermissionRequest[]`.
- `GET /question?directory=...` → `QuestionRequest[]`.

```ts
type PermissionRequest={id:string;sessionID:string;permission:string;patterns:string[];
 metadata:Record<string,unknown>;always:string[];tool?:{messageID:string;callID:string}};
type QuestionRequest={id:string;sessionID:string;questions:QuestionInfo[];
 tool?:{messageID:string;callID:string}};
type QuestionInfo={question:string;header:string;
 options:{label:string;description:string}[];multiple?:boolean;custom?:boolean};
type QuestionAnswer=string[];
```

Replies:

```http
POST /permission/{requestID}/reply?directory=...
{"reply":"once","message":"optional text"}

POST /question/{requestID}/reply?directory=...
{"answers":[["Selected label"],["A","B"]]}

POST /question/{requestID}/reject?directory=...
(no body)
```

Permission `reply` is exactly `once|always|reject`; all three endpoints return `200 boolean` and `404` a typed not-found error. Question answers are ordered one array per question; labels/custom text are strings inside that array.

Important scope: 1.17.18 stores an `always` grant per **directory**, not merely per current tool call. On a shared server it can widen later sessions in that directory. Use `once` unless the user explicitly intends that directory-scoped rule.

### Agents/modes, providers/models, config

| Method/path | response |
|---|---|
| `GET /agent?directory=...` | `Agent[]`. `mode` is `subagent|primary|all`; the built-in plan/build choices are agent names, not a separate mode endpoint. |
| `GET /provider?directory=...` | `{all:Provider[],default:Record<string,string>,connected:string[]}`. |
| `GET /config/providers?directory=...` | `{providers:Provider[],default:Record<string,string>}`. |
| `GET /config?directory=...` | effective `Config`. |
| `PATCH /config?directory=...` | body is `Config`; returns effective `Config`. This mutates configuration and should not be used merely to select one prompt's model. |

```ts
type Agent={name:string;description?:string;mode:"subagent"|"primary"|"all";
 native?:boolean;hidden?:boolean;topP?:number;temperature?:number;color?:string;
 permission:PermissionRuleset;model?:{modelID:string;providerID:string};variant?:string;
 prompt?:string;options:Record<string,unknown>;steps?:number};
type Provider={id:string;name:string;source:"env"|"config"|"custom"|"api";
 env:string[];key?:string;options:Record<string,unknown>;models:Record<string,Model>};
type Model={id:string;providerID:string;api:{id:string;url:string;npm:string};name:string;family?:string;
 capabilities:{temperature:boolean;reasoning:boolean;attachment:boolean;toolcall:boolean;
 input:{text:boolean;audio:boolean;image:boolean;video:boolean;pdf:boolean};
 output:{text:boolean;audio:boolean;image:boolean;video:boolean;pdf:boolean};
 interleaved:boolean|{field:"reasoning"|"reasoning_content"|"reasoning_details"}};
 cost:{input:number;output:number;cache:{read:number;write:number};tiers?:unknown[];experimentalOver200K?:unknown};
 limit:{context:number;input?:number;output:number};status:"alpha"|"beta"|"deprecated"|"active";
 options:Record<string,unknown>;headers:Record<string,string>;release_date:string;
 variants?:Record<string,Record<string,unknown>>};
```

`Config` top-level fields in 1.17.18: `$schema,shell,logLevel,server,command,skills,references,reference,watcher,snapshot,plugin,share,autoshare,autoupdate,disabled_providers,enabled_providers,model,small_model,default_agent,username,mode,agent,provider,mcp,formatter,lsp,instructions,layout,permission,tools,attachment,enterprise,tool_output,compaction,experimental`. The exact nested schema is in the saved OpenAPI; key implementation-relevant shapes are `agent/mode:Record<string,AgentConfig>`, `provider:Record<string,ProviderConfig>`, `permission:PermissionConfig`, `tools:Record<string,boolean>`, and `mcp:Record<string,McpConfig>`. Select a turn with prompt `{agent,model:{providerID,modelID},variant}` rather than PATCHing shared config.

### Parts

Every Part begins `id,sessionID,messageID,type`.

```ts
type TextPart={type:"text";text:string;synthetic?:boolean;ignored?:boolean;
 time?:{start:number;end?:number};metadata?:Record<string,unknown>};
type ReasoningPart={type:"reasoning";text:string;metadata?:Record<string,unknown>;
 time:{start:number;end?:number}};
type SubtaskPart={type:"subtask";prompt:string;description:string;agent:string;
 model?:{providerID:string;modelID:string};command?:string};
type FilePart={type:"file";mime:string;filename?:string;url:string;source?:FilePartSource};
type ToolPart={type:"tool";callID:string;tool:string;state:ToolState;
 metadata?:Record<string,unknown>};
type StepStartPart={type:"step-start";snapshot?:string};
type StepFinishPart={type:"step-finish";reason:string;snapshot?:string;cost:number;
 tokens:{total?:number;input:number;output:number;reasoning:number;cache:{read:number;write:number}}};
type SnapshotPart={type:"snapshot";snapshot:string};
type PatchPart={type:"patch";hash:string;files:string[]};
type AgentPart={type:"agent";name:string;source?:{value:string;start:number;end:number}};
type RetryPart={type:"retry";attempt:number;error:ApiError;time:{created:number}};
type CompactionPart={type:"compaction";auto:boolean;overflow?:boolean;tail_start_id?:string};
```

`FilePartSource`:

```ts
type FileSource={type:"file";text:{value:string;start:number;end:number};path:string};
type SymbolSource={type:"symbol";text:{value:string;start:number;end:number};path:string;
 range:{start:{line:number;character:number};end:{line:number;character:number}};name:string;kind:number};
type ResourceSource={type:"resource";text:{value:string;start:number;end:number};clientName:string;uri:string};
```

`ToolState` is the canonical tool lifecycle:

```ts
type ToolStatePending={status:"pending";input:Record<string,unknown>;raw:string};
type ToolStateRunning={status:"running";input:Record<string,unknown>;title?:string;
 metadata?:Record<string,unknown>;time:{start:number}};
type ToolStateCompleted={status:"completed";input:Record<string,unknown>;output:string;title:string;
 metadata:Record<string,unknown>;time:{start:number;end:number;compacted?:number};attachments?:FilePart[]};
type ToolStateError={status:"error";input:Record<string,unknown>;error:string;
 metadata?:Record<string,unknown>;time:{start:number;end:number}};
```

### `/event` SSE wire

`GET /event?directory=...` returns `text/event-stream`. Wire framing observed:

```text
data: {"id":"evt_...","type":"server.connected","properties":{}}

```

There is no separate `event:` line in the capture. Parse each `data:` payload as one JSON event. The HTTP `/event` event shape is `{id:string,type:string,properties:object}`. Do not accidentally use the SDK's parallel durable internal types that call the payload `data`.

#### Exhaustive 1.17.18 event catalog

The following is the complete `Event` union exposed for `/event`. `?` means optional; `{}` means an open object.

Core conversation events:

```text
models-dev.refreshed             {}
integration.updated              {}
integration.connection.updated   {integrationID}
catalog.updated                  {}
session.created                  {sessionID,info:Session}
session.updated                  {sessionID,info:Session}
session.deleted                  {sessionID,info:Session}
message.updated                  {sessionID,info:Message}
message.removed                  {sessionID,messageID}
message.part.updated             {sessionID,part:Part,time:number}
message.part.delta               {sessionID,messageID,partID,field:string,delta:string}
message.part.removed             {sessionID,messageID,partID}
session.diff                     {sessionID,diff:SnapshotFileDiff[]}
session.error                    {sessionID?,error?:MessageError}
session.status                   {sessionID,status:SessionStatus}
session.idle                     {sessionID}
session.compacted                {sessionID}
todo.updated                     {sessionID,todos:Todo[]}
```

New durable/session-next projection events (also in `/event` union):

```text
session.next.agent.switched      {timestamp,sessionID,messageID,agent}
session.next.model.switched      {timestamp,sessionID,messageID,model:ModelRef}
session.next.moved               {timestamp,sessionID,location:LocationRef,subdirectory?}
session.next.prompted            {timestamp,sessionID,messageID,prompt:Prompt,delivery:"steer"|"queue"}
session.next.prompt.admitted     {timestamp,sessionID,messageID,prompt:Prompt,delivery:"steer"|"queue"}
session.next.context.updated     {timestamp,sessionID,messageID,text}
session.next.synthetic           {timestamp,sessionID,messageID,text}
session.next.shell.started       {timestamp,sessionID,messageID,callID,command}
session.next.shell.ended         {timestamp,sessionID,callID,output}
session.next.step.started        {timestamp,sessionID,assistantMessageID,agent,model:ModelRef,snapshot?}
session.next.step.ended          {timestamp,sessionID,assistantMessageID,finish,cost,tokens:{input,output,reasoning,cache:{read,write}},snapshot?,files?}
session.next.step.failed         {timestamp,sessionID,assistantMessageID,error:SessionErrorUnknown}
session.next.text.started        {timestamp,sessionID,assistantMessageID,textID}
session.next.text.delta          {timestamp,sessionID,assistantMessageID,textID,delta}
session.next.text.ended          {timestamp,sessionID,assistantMessageID,textID,text}
session.next.reasoning.started   {timestamp,sessionID,assistantMessageID,reasoningID,providerMetadata?}
session.next.reasoning.delta     {timestamp,sessionID,assistantMessageID,reasoningID,delta}
session.next.reasoning.ended     {timestamp,sessionID,assistantMessageID,reasoningID,text,providerMetadata?}
session.next.tool.input.started  {timestamp,sessionID,assistantMessageID,callID,name}
session.next.tool.input.delta    {timestamp,sessionID,assistantMessageID,callID,delta}
session.next.tool.input.ended    {timestamp,sessionID,assistantMessageID,callID,text}
session.next.tool.called         {timestamp,sessionID,assistantMessageID,callID,tool,input:{},provider:{executed,metadata?}}
session.next.tool.progress       {timestamp,sessionID,assistantMessageID,callID,structured:{},content:LlmToolContent[]}
session.next.tool.success        {timestamp,sessionID,assistantMessageID,callID,structured:{},content:LlmToolContent[],outputPaths?,result?,provider:{executed,metadata?}}
session.next.tool.failed         {timestamp,sessionID,assistantMessageID,callID,error:SessionErrorUnknown,result?,provider:{executed,metadata?}}
session.next.retried             {timestamp,sessionID,attempt,error:SessionNextRetryError}
session.next.compaction.started  {timestamp,sessionID,messageID,reason:"auto"|"manual"}
session.next.compaction.delta    {timestamp,sessionID,messageID,text}
session.next.compaction.ended    {timestamp,sessionID,messageID,reason:"auto"|"manual",text,recent}
session.next.revert.staged       {timestamp,sessionID,revert:RevertState}
session.next.revert.cleared      {timestamp,sessionID}
session.next.revert.committed    {timestamp,sessionID,messageID}
```

Human gates:

```text
permission.asked       {id,sessionID,permission,patterns:string[],metadata:{},always:string[],tool?:{messageID,callID}}
permission.replied     {sessionID,requestID,reply:"once"|"always"|"reject"}
question.asked         {id,sessionID,questions:QuestionInfo[],tool?:{messageID,callID}}
question.replied       {sessionID,requestID,answers:string[][]}
question.rejected      {sessionID,requestID}
permission.v2.asked    {id,sessionID,action,resources:string[],save?:string[],metadata?:{},source?:{type:"tool",messageID,callID}}
permission.v2.replied  {sessionID,requestID,reply:"once"|"always"|"reject"}
question.v2.asked      {id,sessionID,questions:QuestionInfo[],tool?:{messageID,callID}}
question.v2.replied    {sessionID,requestID,answers:string[][]}
question.v2.rejected   {sessionID,requestID}
```

Other union members:

```text
installation.updated             {version}
installation.update-available    {version}
file.edited                      {file}
reference.updated                {}
plugin.added                     {id}
project.directories.updated      {projectID}
file.watcher.updated             {file,event:"add"|"change"|"unlink"}
pty.created                      {info:Pty}
pty.updated                      {info:Pty}
pty.exited                       {id,exitCode}
pty.deleted                      {id}
lsp.updated                      {}
mcp.tools.changed                {server}
mcp.browser.open.failed          {mcpName,url}
command.executed                 {name,sessionID,arguments,messageID}
project.updated                  {id,worktree,vcs?,name?,icon?,commands?,time,sandboxes:string[]}
tui.prompt.append                {text}
tui.command.execute              {command:string}
tui.toast.show                   {title?,message,variant:"info"|"success"|"warning"|"error",duration?}
tui.session.select               {sessionID}
vcs.branch.updated               {branch?}
workspace.ready                  {name}
workspace.failed                 {message}
workspace.status                 {workspaceID,status:"connected"|"connecting"|"disconnected"|"error"}
worktree.ready                   {name,branch?}
worktree.failed                  {message}
server.connected                 {}
global.disposed                  {}
server.instance.disposed         {directory}
```

### Streaming and completion rules

`SessionStatus`:

```ts
type SessionStatus=
 | {type:"idle"}
 | {type:"busy"}
 | {type:"retry";attempt:number;message:string;
    action?:{reason:string;provider:string;title:string;message:string;label:string;link?:string};
    next:number};
```

- Authoritative turn completion is `session.status` for the session with `status.type:"idle"`. `session.idle` is a compatible edge emitted by this build, but status is the typed level signal. A successful valid `GET /session/status` map with the session absent also means idle.
- A `step-finish` part and `session.next.step.ended` terminate one model/tool step, not necessarily the user turn; more tool/model steps can follow. Never complete a turn from them.
- `session.status busy` is the authoritative execution-start level. Async prompt HTTP 204 means accepted, not started or completed.
- `message.part.delta.properties.delta` is an append delta for `field` (normally text). `message.part.updated.properties.part` is a full current/cumulative Part and must replace by `part.id`. If consuming both, deduplicate/reconcile; never append `TextPart.text` as though it were a delta.
- A `ToolPart` often arrives repeatedly with the same part ID and state transitioning pending→running→completed/error. Upsert, do not create four calls.
- SSE silence is not idle. On a quiet/disconnected stream, poll `/session/status`, then reconcile messages, `/permission`, and `/question`. `/event` has no documented cursor/Last-Event-ID replay parameter; reconnect with `server.connected` and treat REST snapshots as recovery truth.
- `session.error` does not itself prove idle; wait for idle/status reconciliation. Conversely idle can follow an error, as the fixture demonstrates.

### OpenCode fixtures

- `fixtures/opencode-openapi.json`: raw live `/doc` from installed 1.17.18 (478,565 bytes, 162 paths).
- `fixtures/opencode-events.ndjson`: raw curl SSE framing (`data:` lines plus blank separators), not normalized JSON despite filename. It includes connected, session/message/part updates, busy, error, idle, and `session.idle`.
- `fixtures/opencode-session.json`, `opencode-prompt-response.json`, `opencode-status-latest.json`, and `opencode-messages.json`: raw REST captures.
- The single prompt returned HTTP 500 because configured default `openai/gpt-5.6-sol` was rejected: `Model not found: openai/gpt-5.6-sol`. The user message persisted, `session.error` carried the specific error, status returned to idle, and the final status map was `{}` (idle session omitted). No retry was made.

## Adapter contract implications

Use these as provider-specific authoritative signals. “Authoritative” means the signal that commits adapter state; earlier events may be used for optimistic UI only.

| Contract event | Claude Code 2.1.266 | OpenCode 1.17.18 (historical) |
|---|---|---|
| Turn started | Host request is accepted when the NDJSON user line is written. CLI-side start is `system/init` (documented at each turn start), or the first response frame stamped with the submitted `user_message_uuid`; `--replay-user-messages` supplies an explicit stdout user acknowledgement if desired. | `session.status {status:{type:"busy"}}` for the target session. `prompt_async` 204 is admission only; sync request being open is not a wire start signal. |
| Streaming delta | `stream_event/event.type:"content_block_delta"`; append only `text_delta.text` for assistant text. Buffer `input_json_delta.partial_json` for tool input and route thinking/citation deltas separately. | `message.part.delta {field,delta}` or the newer `session.next.text.delta`/reasoning/tool-input deltas. `message.part.updated.part` is cumulative replacement, not append. |
| Tool call started | Early: `stream_event content_block_start` whose block is `tool_use`; canonical call identity/input: completed `assistant.message.content[]` `tool_use {id,name,input}`. `tool_progress` is heartbeat/progress, not creation. | Upsert `message.part.updated` `ToolPart` at `state.status:"pending"|"running"` (running means execution began). New projection equivalent: `session.next.tool.called`; input construction has its own started/delta/ended events. |
| Tool call finished | stdout `user` containing matching `tool_result.tool_use_id`; inspect block `is_error` and structured `tool_use_result`. | Same `ToolPart.callID` transitions to `state.status:"completed"|"error"`. New projection equivalent: `session.next.tool.success|failed`. |
| Permission requested | stdout `control_request` with `request.subtype:"can_use_tool"`. If prompting is not configured, no request is guaranteed; auto-deny advisory is `system/permission_denied`, final truth is `result.permission_denials`. | `permission.asked` (current legacy surface) or `permission.v2.asked`; recover missed requests with `GET /permission`. |
| Question asked | `can_use_tool` with `tool_name:"AskUserQuestion"`; key returned answers by exact question text. MCP/user-dialog questions use `elicitation` or declared `request_user_dialog` instead. | `question.asked` or `question.v2.asked`; recover with `GET /question`. Reply uses ordered `string[][]`. |
| Plan proposed | Assistant `tool_use` and/or `can_use_tool` with `tool_name:"ExitPlanMode"`; capture `input.plan` when present. Allow means proceed to execution; deny-with-handoff means proposal delivered and wait. | No dedicated plan-proposed event. The host knows it selected `agent:"plan"`; treat the completed assistant text at session idle as the proposal. `todo.updated` is plan/progress structure, not proposal completion. |
| Turn completed | Exactly one `type:"result"` for the turn. Inspect `subtype`, `is_error`, `terminal_reason`, `permission_denials`, and `queued_turn_count`. Do not use EOF. | `session.status` with `idle`, or a valid `/session/status` map from which the session is absent. `session.idle` is a compatible edge. Reconcile final messages/errors. Never use step-finish. |
| Turn aborted | Send `interrupt`; receipt acknowledges control. The correlated turn's later `result`, interpreted with outstanding interrupt and abort terminal reason, settles the turn. It is not settled by the control response alone. | `POST /session/{id}/abort` boolean acknowledges request; later idle settles execution. Classify aborted from the locally outstanding abort and/or `AssistantMessage.error.name:"MessageAbortedError"`; there is no dedicated `session.aborted` event in this union. |
| Error | Turn terminal: error result subtype or success subtype with `is_error:true`; also record `assistant.error`. `api_retry` is nonterminal. | `session.error` and/or `AssistantMessage.error`; HTTP non-2xx is request failure but may coexist with persisted user/event state. Wait/reconcile idle. Retry status is nonterminal. |
| Process/server death | Child exit or stdout EOF before expected result is runtime death. `worker_shutting_down` is only a live-tail hint and may be absent/replayed. | Supervised `serve` child exit is primary. SSE EOF alone means subscription loss; confirm with child status/health request, then mark server death. |
| Resume | Spawn a new process with `--resume=SESSION_ID` (and optional fork flags). Verify returned `system/init.session_id`; cost counters restart for the process. | No resume RPC: retain `(base_url,directory,sessionID)`, `GET /session/{id}`, reconnect `/event`, then fetch messages/status/pending gates. Fork is explicit POST and returns a new session. |

Implementation invariants:

1. Persist provider session ID, turn correlation IDs, message/part/tool IDs, and the last normalized state independently of either live transport.
2. Parse unknown variants losslessly into diagnostics while continuing the stream.
3. Serialize replies through a single writer per Claude stdin and a single request-ID registry; reject duplicate local settlements idempotently.
4. For OpenCode, treat SSE as an acceleration channel and REST snapshots as reconnect reconciliation. For Claude, treat each `result` as the turn commit and process exit as a separate session-liveness event.
