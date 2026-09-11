# 0014 — Drop OpenCode, add Codex

**Adopted** for `fleet-core::agents`, `fleet-daemon`'s `services/agents/`, `fleet-proto`'s agent
family, and the `config.agentCommands` schema. Fleet supports exactly two harnesses: **Claude
Code** and **Codex**. OpenCode is deleted, not deprecated. `docs/NATIVE-AGENTS.md` §4 is the
specification; this file records why, and what the removal costs.

**Built.** `crates/fleet-daemon/src/agents/codex/**` speaks the app-server protocol against the
installed `codex-cli` 0.147.0, with `wire/` and `methods.rs` generated from the binary's own
schema by `scripts/generate-codex-wire.py`; `providers/opencode/**` is deleted. This ADR recorded
the decision before the work and now records what shipped.

**Amends [ADR 0010](0010-native-agents.md),** which adopted Claude Code + OpenCode. Everything in
0010 about speaking the structured protocol, completion authority, the pure reducer in
`fleet-core`, and the daemon owning the process remains in force. Only the second harness changes,
and the one OpenCode-specific choice in 0010 — "one managed `opencode serve` per thread" — is
withdrawn with it.

## Why Codex instead of OpenCode

- **Codex's protocol is a superset of what Fleet's UI needs, and OpenCode's is not.** The
  app-server vocabulary is `thread/start`, `turn/start`, `turn/steer`, `turn/interrupt`,
  `item/started`, `item/completed` — the same nouns Fleet's own event model already uses. It ships
  a native `turn/steer` guarded by `expectedTurnId`, first-class approval requests per action kind,
  native model questions (`item/tool/requestUserInput`) with `isSecret` and `isBlocking`, a
  turn-level unified diff (`turn/diff/updated`), live token usage with the context-window
  denominator, per-model reasoning-effort sets from `model/list`, and `ThreadStatus.activeFlags`
  that hands Fleet its attention state directly. OpenCode required Fleet to derive most of that.
- **A generated protocol beats a hand-read one.** `codex app-server generate-json-schema
  --experimental` emits the authoritative schema for the installed binary — 625 types in the v2
  namespace. Fleet checks in generated `wire.rs` and `methods.rs`, because hand-transcribing 99
  request methods, 72 notifications and 18 item variants rots inside one Codex release. OpenCode
  offered an OpenAPI document but the semantics that mattered — cumulative-replace vs append,
  which signal settles a turn — were not in it.
- **One managed server process per thread was a real cost.** OpenCode needed a supervised
  `opencode serve` per thread with a free port, a readiness probe, a process group and a two-stage
  kill, because MCP and directory registration are server-wide while the working directory belongs
  to the thread. Codex is a child process on stdio, like Claude. That collapses two transport
  designs into one shape and removes a whole class of port, readiness and orphaned-server failures.
- **SSE silence was never idle.** OpenCode's completion authority required polling
  `/session/status` on a quiet stream with backoff, then reconciling messages, permissions and
  questions as recovery truth. Codex's `turn/completed` is a single terminal notification on the
  same stream as everything else, which is the same shape as Claude's single `result`.
- **Two harnesses that share a transport shape share an implementation.** Line framing, the
  pending-request registry, the settle-before-kill teardown order, the tolerant-decode rules and
  the lifecycle guard are one implementation each rather than two.

Codex's own costs are recorded rather than hidden: it is a JSON-RPC-*shaped* protocol that is not
JSON-RPC 2.0 (no `jsonrpc` field, `params` omitted when absent), it multiplexes subagent threads
onto one connection and needs a routing table with three guards, it adds enum values without a
version bump, and it has no protocol version at all — the CLI version is scraped from
`initialize.userAgent`. `docs/NATIVE-AGENTS.md` §4.2 and §4.5 carry the mitigations.

## What the removal costs

- **`crates/fleet-daemon/src/services/agents/providers/opencode/`** — 3 502 lines, deleted in the
  same commit that lands the new `Harness` trait.
- **`AgentKind` loses a variant.** It is a closed two-arm enum wired into four places: the
  persisted `agents/index.json` (superseded by ADR 0013's schema, which is where the migration
  happens), `Snapshot::agent_threads` on the wire, `config.agentCommands`, and
  `AgentBinaries`/`AgentCommands`. `Capabilities` — a fixed six-bool struct that cannot describe
  Codex's three sandbox axes or two reasoning channels — is replaced by
  `HarnessCapabilities` with a `ControlCost` per control.
- **The persisted and wire schemas need a deprecation path, not a delete.** `config::Agent::Opencode`
  and the `opencode` fields on `AgentBinaries`/`AgentCommands` are read with `#[serde(other)]`
  tolerance for one release: an existing config naming OpenCode decodes, the thread refuses to
  start with a typed `Unavailable` naming the removal and the `^s F` fallback, and the field is
  dropped in the release after. A hard decode failure would make `fleetd` refuse to start on a
  config that was valid yesterday.
- **OpenCode's wire reference is kept, not deleted.** `docs/research/harness-protocols.md` moves
  its OpenCode half to an appendix marked historical. It is the record of what was verified, and
  deleting verified research to make a document tidier destroys the only evidence that the
  decisions above were informed.

## Why a capability string and not a protocol bump

`PROTOCOL_VERSION` stays at **7**.

The daemon matches the protocol version *literally* and a remote link requires an exact match, so
bumping to 8 would force every remote daemon in a fleet to upgrade in lockstep — a local app that
learned about Codex could no longer talk to a remote host that had not. That is the opposite of
what adding a harness should cost.

So Codex support is announced as an **additive capability string** on the existing handshake —
`agent.codex`, one of the six in `fleet_proto::AGENT_CAPABILITIES` — every new field is additive
with `#[serde(default)]`, and every new enum arm has a `#[serde(other)]` fallback. A mixed-version
fleet keeps working: a host without the capability simply does not offer Codex threads, and the
client's picker does not draw the option. This is `rust-ipc-protocol`'s rule applied to a harness
rather than to a wire field, and it is the same reason §4.5 gates harness features on declared
capabilities rather than on version-string comparisons.

`snapshot::AgentBinaries` gained a defaulted `codex` alongside `claude` and the surviving
`opencode`, and the login-shell probe behind `fleet hosts` / `fleet doctor` looks for all three.
The field is additive for the same reason as the capability: an older daemon answers without it,
and `false` is the honest reading — it cannot run a Codex thread whether or not the binary is on
its `PATH`.
