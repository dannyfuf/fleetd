# 0017 — Native subagents are delegated native threads

**Adopted** for `fleet-core::agents`, `fleet-daemon`'s
`services/agents/delegation/`, `fleet-proto`'s agent family, and `fleet-cli`.
A subagent is a native Claude Code or Codex thread plus a durable `Delegation`
record connecting it to a caller. The daemon owns the lifecycle and delivery;
`fleet subagent` is the spawn and reporting surface.

**Built.** Phases 1–6 shipped in this branch: phase 1 split new turns from
steers, preserved background tasks, and recorded stop causes; phase 2 added the
delegation types, wire family, transcript origins, and SQLite schema; phase 3
shipped the daemon service, durable outbox, and `fleet subagent` CLI; phase 4
added delegation rows, result cards, child tabs, and attach/detach; phase 5
added the agents picker; and phase 6 added one-shot recovery, undeliverable
results, and cancellation through the delegation tree.

**Amends [ADR 0010](0010-native-agents.md)** by allowing one native agent
thread to create another native agent thread and by making that relationship
visible in the transcript and app. Its structured-protocol, daemon-ownership,
completion-authority, and pure-reducer decisions remain in force.

**Amends [ADR 0013](0013-sqlite-agent-transcripts.md)** by adding the derived
`delegations` and `delegation_outbox` tables and parent, delegation, and stop
cause columns on thread records. They are updated in the same SQLite
transactions as the agent events from which lifecycle transitions follow.

## A delegation is a thread plus a durable relationship

The child is an ordinary native thread. It has the same provider adapter,
structured event stream, transcript, gates, controls, persistence, recovery,
and inspectable app surface as a thread created by a person. It can itself be
a caller, so nesting does not introduce a second kind of agent runtime.

The `Delegation` is the relationship the thread alone cannot express. It
records the caller turn and transcript item, child, provider, depth, brief,
expectation, lifecycle status, result, delivery state, recovery counts, and
timestamps. The caller transcript contains a delegation item, and the child
record points back to its parent and delegation. Either side can therefore be
reconstructed after a daemon restart.

`JobManager` was rejected as the owner of subagents. A job is a process and
output lifecycle; a native child is a structured conversation with turns,
gates, provider state, resumability, and a transcript. Treating it as a job
would either discard those semantics or duplicate the native-agent manager
inside the jobs service. Jobs may later show a read-only projection, but they
are not the source of truth.

## The spawn surface is a CLI, not MCP

A caller runs `fleet subagent run` through the shell tool it already has. The
command speaks Fleet's typed protocol to the daemon, returns as soon as the
child and durable record exist, and leaves the daemon to wait. The child later
runs `fleet subagent complete`; callers never poll in a turn. The remaining
`status`, `list`, `wait`, and `cancel` verbs are useful to people and harnesses
through the same interface.

MCP was rejected as the primary surface because it would require provider- and
installation-specific server configuration before delegation could work. Both
supported providers already have a shell, and the CLI preserves one validation,
authentication, wire, and output path. An MCP server can wrap these commands
later without changing the daemon model or making MCP the source of truth.

## “Done” requires three independent facts

A successful delegation needs all three of these facts:

1. The child reported a result with `fleet subagent complete` using the right
   delegation identity and token.
2. The provider emitted its authoritative `TurnSettled { Completed }`; a report
   command alone cannot declare the native turn finished.
3. No question, plan, or permission gate remains open.

A live background task adds one grace rule. If the completed turn still has
background work, the delegation enters `Settling`. The worker waits until no
background task is live or the 30-second grace expires, then succeeds it. This
prevents ordinary asynchronous cleanup from racing result delivery while also
preventing a leaked task from holding the delegation forever.

A child that settles without reporting is nudged at most twice. If it still
does not report, the delegation ends `Incomplete` with the latest assistant
text as a fallback. Provider errors, exhausted turns, denied turns, and a
reported block end as failures; interruption and explicit cancellation end as
cancelled. Fleet does not infer success from silence, process exit, or a report
command in isolation.

## Correctness is SQLite plus an outbox, not the event bus

The child-side transition runs in the same SQLite transaction that appends and
projects the triggering agent event. That transaction updates the delegation
and enqueues durable follow-up actions such as mirror, nudge, settle, recover,
deliver, and cancel descendants. It performs no manager call while the writer
transaction is open. After commit, the worker drains the outbox on wake, on a
retry interval, and once at daemon start.

Delivery becomes durable in the transaction that records the caller's new
`UserMessage` with `origin: Delegation`: that transaction stores its sequence
and turn on the delegation and closes the deliver action. A crash before that
commit leaves the action open for retry; a crash after it finds delivery
recorded. The broadcast bus may lag or drop messages, so it is used only to
refresh clients with `DelegationChanged`, never to drive a state transition or
guarantee delivery.

## Reporting is authorized by a per-delegation token

The daemon creates a 64-hex-character random token and gives it only to the
child through `FLEET_DELEGATION_TOKEN`, alongside the delegation and thread
identities. SQLite stores only its SHA-256 digest. `complete` must present the
delegation ID, the matching child thread, and a token whose digest matches in
constant time. The token is a narrow capability to report this delegation's
result; possession of a thread ID alone is not authority to complete it.

## Delivery waits for the caller unless eagerness is explicit

By default a terminal result waits until the caller is idle, then arrives as a
new user message with delegation origin. It also waits while the caller has an
open gate. `fleet subagent run --eager` is the explicit choice to deliver into
a running caller through the normal send/steer path.

A caller lost to a provider exit may be resumed through its cursor for
delivery. A caller explicitly stopped by the user is never auto-resumed; the
result remains pending until the caller is deliberately reopened. If the
caller no longer exists or cannot be resumed, the delegation becomes
`Undeliverable` with a readable reason rather than silently losing the result.

*Amended 2026-09-19:* end-of-turn injection remains the default, but a caller
that has **already taken the result through its own `fleet subagent wait`**
consumes the delivery instead of receiving it twice. The reason injection
exists is that the caller has not seen the result; a caller that waited has.
`DelegationWait` therefore carries an optional `caller`, and a wait whose
caller equals the delegation's caller marks the delivery `Consumed` in the same
write that answers it. The delivery worker still patches the caller's
transcript item terminally and still closes its outbox row — the record must
stop saying "working" — and only the user message is skipped. The wire field is
advisory identity, not authorisation: it decides whether a delivery is
consumed, never whether a wait is answered, so an older client, a wait from a
shell with no `FLEET_SESSION`, and a wait from a third party all consume
nothing and keep today's behaviour exactly. The exactly-once boundary is not
weakened by this, it is restated: a result reaches its caller at most once, by
whichever of `wait` and the worker reaches it first. What forced the amendment
was an orchestrator that waited on eight children and then received all eight
results again as user messages when its own turn settled — eight duplicate
briefs' worth of context spent to learn nothing new.

## Three product defaults are deliberate

- **The caller's worktree is the default.** It makes the common delegation
  cheap and lets the child work on the caller's task without setup. Fleet warns
  that both threads share files; `--worktree` selects another worktree when
  isolation matters.

  *Amended 2026-09-19:* the warning fires only for the **implicit** default. A
  caller that passed `--worktree` gets none, even when the worktree it named is
  its own. Passing the flag is evidence the sharing was considered, and the
  pattern it names is real: an orchestrator that runs several children in one
  worktree under disjoint file ownership does exactly this. A warning that
  cannot be silenced by the deliberate choice it is warning about is a warning
  that stops being read, which costs more than the case it was guarding.
- **A user stop is final until a user reverses it.** Fleet distinguishes
  `StopCause::User` from `StopCause::ProviderExit`. Only the latter is eligible
  for automatic recovery, and a child gets at most one recovery nudge before a
  second provider exit becomes failure.
- **Full access, not inherited restriction, is the standing default.** Children
  start in `PermissionMode::FullAccess` for both providers unless the caller
  explicitly passes `--mode`. Delegated work is unattended, so silently
  inheriting an interactive caller's permission gates would strand it; an
  explicit narrower mode remains available when the caller accepts that risk.

## Limits bound unattended work

Nesting is capped at depth 3, one caller may have at most 4 live children, and
one daemon may have at most 8 live delegations. A child receives at most 2
missing-report nudges and one provider-exit recovery. Result text is capped at
the transcript item-body budget (256 KiB) and records when it was elided.
`fleet subagent wait` defaults to 540 seconds so a shell call does not outlive
the provider's own tool ceiling. Remote-host delegation is refused rather than
pretending local worktree, process, and token assumptions hold across daemons.

*Amended 2026-09-19:* 540 seconds remains the **default**, for the same reason
it was chosen — it sits under the Claude Code shell-tool ceiling — but it is no
longer a **cap**. Fleet imposes no upper bound of its own on `--timeout`; the
caller's own tool timeout may still kill a longer wait, which is the caller's
ceiling to know and not Fleet's to guess. The original reasoning held for one
caller whose ceiling Fleet happened to know, and refusing every other caller's
longer wait bought nothing: nothing below the CLI enforced the cap, so it was a
clap range rejecting a request the protocol and the daemon would both have
served. What the cap was really protecting against — a wait that times out and
reports as if the child had finished — is fixed where it belongs, in the message:
a timed-out wait exits 2 with a distinct non-terminal line naming the
delegation, its status and the elapsed wait.

## Deferred, not implied

This decision does not ship an MCP wrapper around the CLI, richer UX for a
terminal-origin caller beyond `--caller`, remote-host delegations, structured
JSON result semantics, a way for the caller to answer a child's gates through
the CLI, or a Jobs-screen projection. Each can be added on top of the durable
thread-and-delegation model; none is required for the model's correctness.
