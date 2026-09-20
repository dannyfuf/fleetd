# `fleet subagent` and `fleet agent tail` command reference

Source of truth: `crates/fleet-cli/src/args.rs` (`SubagentCommand`, `SubagentRunArgs`,
`SubagentCompleteArgs`, `SubagentWaitArgs`, `AgentTailArgs`), `crates/fleet-cli/src/commands/subagents.rs`,
and `docs/NATIVE-AGENTS.md` §15.3 and §15.4. When this file and the code disagree, the code is
right and this file needs the fix.

## Environment the daemon sets

| Variable | Who has it | Meaning |
| --- | --- | --- |
| `FLEET_SESSION` | every native thread | The thread's UUID. `run` and `wait` use it as the default caller; `complete` requires it to be the child. |
| `FLEET_DELEGATION` | delegated children only | The delegation id `complete` defaults to. Its presence means "I am a child". |
| `FLEET_DELEGATION_TOKEN` | delegated children only | Bearer token `complete` presents. Never echo, forward, or set it. |

`--env` refuses any `FLEET_*` key and `PATH`. Fleet prepends the directory of a compatible
`fleet` to the child's `PATH` itself.

## `fleet subagent run`

Starts a child. Returns as soon as the child thread and the durable record exist. Only a native
thread that is mid-turn may call it (the daemon rejects a caller that is idle or not a thread).

| Flag | Required | Meaning |
| --- | --- | --- |
| `--provider claude\|codex` | yes | Which harness runs the child. |
| `--expect <text>` | yes | Completion criteria, printed to the child as `The caller expects: …`. |
| `--brief-file <path>` | no | The brief. Omitted, the brief is read from stdin to EOF. |
| `--worktree <owner/name#slug>` | no | The child's worktree. Default: the caller's, with a warning. Passing the caller's own id explicitly silences the warning. |
| `--mode ask\|accept-edits\|plan\|auto\|dont-ask\|full-access` | no | Permission mode. Default resolves through the harness default, `full-access` when unset. |
| `--model <name>` | no | Provider-native model. A blank value is a validation error. |
| `--effort <text>` | no | Provider-native reasoning effort, free text, passed through unvalidated. Works without `--model`. |
| `--title <text>` | no | Child thread title. Default `↳ <provider> — <first 48 chars of the brief>`. |
| `--env KEY=VALUE` | no, repeatable | Whole-value environment override for the child. Refused: no `=`, empty key, duplicate key, `FLEET_*`, `PATH`. Persisted with the delegation so a resumed child keeps it. Never echoed anywhere. |
| `--eager` | no | Deliver the result into a running caller turn instead of waiting for idle. |
| `--caller <thread>` | no | Calling thread; defaults to `FLEET_SESSION`. |
| `--json` | no | Envelope with `delegation` (brief cut to 200 chars, `briefElided: true` when cut) and optional `warning`. |

Human output: `delegation <id> started, child thread <child>`, plus the warning line when the
worktree defaulted.

Refusals name their rule: `running-turn rule` (the caller has no running turn),
`depth-limit rule` (caller at depth 3), `live-child-limit rule` (4 live children),
`daemon-live-limit rule` (8 live delegations), and a remote-mirrored caller.

## `fleet subagent complete [<id>]`

Run by the child, exactly once, after the work is verified and the report is written.

| Flag | Meaning |
| --- | --- |
| `<id>` | Delegation id; defaults to `FLEET_DELEGATION`. |
| `--result-file <path>` | The report. Omitted, it is read from stdin to EOF. |
| `--blocked` | Store the report and end the delegation `failed` when the turn settles; the report should say what is needed. |
| `--json-result` | Refuse a report that is not valid JSON. Use only when the caller asked for JSON. |
| `--json` | Envelope with the whole delegation record. Otherwise prints `reported`. |

Requires `FLEET_SESSION` (the child thread) and `FLEET_DELEGATION_TOKEN`. A report over 256 KiB
is truncated at ingest, marked elided, and a notice is printed to stderr. A second `complete`
with a different body is refused; an identical one succeeds idempotently. Any `complete`
against a terminal delegation is refused.

## `fleet subagent wait <id>`

Blocks until the delegation is terminal or the timeout elapses.

| Flag | Meaning |
| --- | --- |
| `--timeout <seconds>` | Default 540, chosen to sit under the Claude Code shell-tool ceiling. No upper bound; the caller's own tool timeout still applies. |
| `--caller <thread>` | The waiting thread; defaults to `FLEET_SESSION`. When it equals the delegation's caller, a terminal answer consumes the delivery so the caller is not also sent the report as a user message. Any other value, or none, consumes nothing. A malformed `FLEET_SESSION` is refused. |
| `--json` | Envelope as `run` (brief elided). `delegation.result.text` is the report body. Identical shape for terminal and timed-out answers; read `delegation.status`. |

| Exit | Output |
| --- | --- |
| 0 | The delivered message: `[fleet subagent <id> finished: <status>]`, a provider/thread/duration/files line, a blank line, the report. `(report elided at <n> bytes)` follows an elided one. |
| 2 | One line: `[fleet subagent <id> still running after <age>, status: <status>, thread: <child>]`. |
| 1 | An error (unknown id, malformed caller, daemon refusal). |

## `fleet subagent status <id>`

Prints the whole record and never consumes anything: the eight-field line, `brief:` in full, a
`usage:` line when the daemon knows the child's spend (tokens in/out/cache, context %, cost),
and, once terminal, the delivered message byte-for-byte as `wait` prints it. `--json` keeps the
brief whole. This is how a missed `wait` is recovered.

## `fleet subagent list [--caller <thread>]`

One line per delegation, eight tab-separated fields:

```text
<id>  <status>  <provider>  <child thread>  <duration>  <total tokens|->  <cost|->  <delivery>
```

`--json` elides every brief over 200 characters and sets `briefElided` if any was cut. Usage is
the child's own thread only, never its descendants.

## `fleet subagent cancel <id>`

Refuses an already-terminal record. Cancels every live descendant depth-first, then interrupts
and stops the child. The caller receives a `cancelled` delivery. Prints `cancelled`; `--json`
prints the whole record.

## `fleet agent tail <child-thread> [--replay] [--no-follow] [--last N]`

The raw event stream of a thread. `--no-follow` prints the retained snapshot and exits (implies
`--replay`); `--last N` trims that snapshot to the last N events. Use it to see what a child is
doing, not to read its report; `status` prints the report.

## Status and delivery vocabulary

| Status | Meaning |
| --- | --- |
| `starting` | Child process launching. |
| `running` | Child working. |
| `blocked` | Child has an open question, plan, or permission gate, or reported `--blocked`. |
| `settling` | Turn completed; waiting for a report nudge or for background work (30 s grace). |
| `succeeded` | Reported, turn completed, no open gate, no live background work. |
| `incomplete` | Turn completed without a report after two nudges; result is the last assistant text (first 4 KiB). |
| `failed` | `--blocked` report, provider error, exhausted turns, denied turn, or a second provider exit. |
| `cancelled` | Interrupted, stopped, or `cancel`led. |

| Delivery | Meaning |
| --- | --- |
| `pending` | Terminal result not yet handed to the caller. |
| `delivered` | Injected into the caller as a user message. |
| `consumed` | The caller took it through its own `wait`; no message will follow. |
| `undeliverable` | The caller is gone or cannot be resumed; `status` shows the reason. |

## Limits

| Limit | Value |
| --- | --- |
| Nesting depth | 3 |
| Live children per caller | 4 |
| Live delegations per daemon | 8 |
| Missing-report nudges | 2 |
| Provider-exit recoveries | 1 |
| Settle grace for background work | 30 s |
| Result body | 256 KiB, truncated at ingest |
| Default `wait` timeout | 540 s, no ceiling |

## Exact child copy

Appended to the child's first message after the brief and one blank line (`{fleet}` is an
absolute path when the daemon resolved one):

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

Missing-report nudge: `You have not reported a result. If the work is done, run \`fleet subagent complete --result-file <path>\`. If not, continue.`

Recovery nudge: `The session was restarted. Continue, and report with \`fleet subagent complete\` when done.`

Sharing warning: `no --worktree was passed, so the child edits the caller's worktree by default; end your turn before it works, or pass --worktree to isolate it`
