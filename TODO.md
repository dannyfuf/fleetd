# TODO — native agents

Everything the native-agent rebuild owes, in one place, ordered by what a user notices first.

`docs/NATIVE-AGENTS.md` §13 is authoritative for *status*; this file is the working list and says
what each item costs. Each entry names the file to start in, why it was left, and how to know it
is done. Nothing here is a known-broken surface: every one of these either says so where the user
meets it, or degrades to a narrower behaviour rather than a wrong one.

---

## 1. Done — a Codex file-change approval joins its diff

Closed in the native-agents round 2 adapter work.

`docs/NATIVE-AGENTS.md` §6.2 promises the approval card shows the diff — *"For an edit approval
Fleet shows what t3code cannot: **the diff**, joined by `itemId`"*. Codex now does.

Codex's `item/fileChange/requestApproval` deliberately carries **no diff and no paths**; the
changes live on the `fileChange` item the request names by `itemId` (see
`docs/research/harness-codex-app-server.md` §3.2). So the card has to join the approval onto that
item. `GateKind::Permission.item` now carries the normalized `ItemId`, the Codex adapter caches
the named file-change paths for human copy, and the thread view joins the diff by that exact id.
A gate that wins the race with its item says `loading edit details…` rather than drawing an empty
card. The additive `agents.threads[].decision` harness projection identifies the joined paths and
whether the drawer rendered a diff; `scenarios/agents/codex-approval-shows-the-diff.scenario`
pins the shipped behaviour in the regular corpus.

## 4. `[u] revert this edit` on a tool row

**Impact: medium.** `[u] revert turn` works; the per-file variant does not.

A file-scope checkpoint names the **turn** it was taken in, not the item, so a tool row has
nothing to key on. Note the capture is best-effort *by construction*: neither harness waits for
Fleet before running an auto-approved tool. The turn-scope checkpoint is the guarantee; the
file-scope one is a finer-grained revert for when the race goes Fleet's way, which it always does
for a gated edit.

- **Start in:** `crates/fleet-daemon/src/services/checkpoints/` — carry the `ItemId` on a
  file-scope checkpoint.
- **Done when:** `u` on an edit row restores that file only, and is not drawn on a row whose edit
  was never checkpointed.

## 5. Attachments are not built

**Impact: medium.** You cannot paste an image into the composer.

`item_attachments` has no writer. The adapter seam can carry an attachments directory, but the
manager currently passes `None`, so Claude is not granted `--add-dir`. §7.2 documents Claude's
block-order rule (images before the final text block, because the CLI
only reads a streamed user message as a slash-command invocation when the last block is text), so
the design is settled — only the implementation is missing.

- **Start in:** `crates/fleet-ui-kit/src/components/agent/` for the pending-chip model, then the
  store writer, the per-thread attachment directory, and both adapters' `UserInput` mapping.
  Codex takes structured `image` /
  `localImage` parts; Claude takes base64 blocks limited to gif/jpeg/png/webp.
- **Limits, already decided:** 120 000 input chars, 8 attachments, 10 MiB per image, 50 MiB per
  file.

## 6. A windowed open of a cold thread still replays its log once

**Impact: low, until a thread gets long.** A latency budget miss, not a wrong result.

The window's *content* comes from the reducer's projection rather than the SQL-native window read
of spec-C §C.2.5, so opening a cold thread replays its log once to build that projection. The
budget in §9.4 (≤ 80 ms for a full window) holds for ordinary threads and degrades on very long
ones.

Also `mirror_oldest_seq` stays `NULL`, because the mirror only ever stores prefixes from sequence
1 — correct today, and the column is there for when it does not.

- **Start in:** `crates/fleet-daemon/src/services/agents/store/read.rs`.
- **Done when:** the row-17 test (500 generated threads, list answered without reading a log)
  extends to a windowed open, measured.

## 7. The raw NDJSON debug log

**Impact: low for users, high for whoever debugs the next protocol drift.**

`RawRef.offset` is always `None`. §14 calls this log the mitigation for protocol drift, and every
wire nuance in §4 was found by reading one. Bound it at 10 MiB × 10 files per thread, keep it
behind a flag, and keep `HarnessEvent.raw` a pointer into it — never an inline payload, which is
what makes a snapshot undecodable at 16 MiB.

- **Start in:** `crates/fleet-daemon/src/agents/harness/`.

## 8. Per-instance harness homes

**Impact: none today.** `HarnessConfig.home` is plumbed and nothing sets it.

Multi-account is explicitly out of scope (§14). When it is wanted: isolate Claude with
`CLAUDE_CONFIG_DIR` and **never** by overriding `HOME` — on macOS that relocates the login
keychain and the CLI reports "Not logged in". `CODEX_HOME` must be tilde-expanded manually,
because `spawn` performs no shell expansion of env values.

## 9. An MCP wrapper around `fleet subagent`

**Impact: low.** An MCP-only agent cannot delegate unless it can also invoke the Fleet CLI.

The CLI is the spawn surface and owns the contract; an MCP server should wrap its six verbs rather
than grow a second delegation client or state machine. This was left until the command and JSON
envelopes were stable enough to be a useful boundary.

- **Start in:** `crates/fleet-cli/src/commands/subagents.rs` and
  `crates/fleet-cli/src/envelope.rs` — treat their commands and JSON output as the adapter contract.
- **Done when:** an MCP client can run, complete, wait for, inspect, list and cancel delegations
  through the CLI, with conformance tests proving the wrapper preserves CLI errors and results.

## 10. Terminal caller UX beyond `--caller`

**Impact: low.** A shell outside a harness can delegate, but the user must already know and paste
the caller thread id.

`fleet subagent run --caller <thread>` is accepted and validated exactly like `FLEET_SESSION`.
Discovery, selection and a friendly terminal workflow were deliberately left out of the first
contract.

- **Start in:** `crates/fleet-cli/src/commands/subagents.rs`, building on the existing agent list
  output instead of inventing another source of thread identity.
- **Done when:** a terminal user can discover and select an eligible running caller without copying
  an opaque id, while `--caller` remains the deterministic non-interactive path.

## 11. Remote-host delegations

**Impact: medium for multi-host users.** Delegation from a mirrored caller is refused today with
`unsupported`: the caller is a mirror, so this daemon cannot safely mutate its turn or deliver the
child's result.

Supporting this requires routing creation, status and delivery through the caller's owning host;
silently creating a local child would split the durable record from the authoritative transcript.

A **card** caller has the same boundary from the other side: `run_for_card` refuses a worktree
another host owns with `automation is unavailable on a worktree owned by host <id>`, because a run
edits a checkout this daemon must be able to reach (`docs/BOARD.md` §11.4). Both refusals are the
same missing piece — host-routed delegation — and either one being lifted should lift the other.

- **Start in:** `crates/fleet-daemon/src/services/agents/delegation/run.rs`, at the mirrored-caller
  refusal, then the existing host-routing boundary.
- **Done when:** a mirrored caller delegates on its owning host and every status transition and
  result is durably reflected back, with disconnect and retry tests proving there is one record and
  one delivery.

## 12. Structured delegation results

**Impact: low.** `fleet subagent complete --json-result` validates JSON, but the daemon still stores
and delivers it as result text, so consumers must parse it again and no result schema is promised.

The initial contract keeps one bounded text result for both human and machine callers. A typed
result needs an additive protocol and persistence shape, not a flag that changes the meaning of the
existing field.

- **Start in:** `fleet-core`'s `DelegationResult`, then the delegation request and CLI envelope
  shapes in `fleet-proto` and `fleet-cli`.
- **Done when:** a child can submit a versioned structured payload that survives storage, query and
  delivery without reparsing text, while existing text results remain compatible.

## 13. The caller answers a child's gates through the CLI

**Impact: medium.** A terminal-only caller can start and wait for a child, but cannot answer the
child's permission or question gates from that same workflow.

The first CLI surface controls the delegation lifecycle only. Gate discovery and answers need to
retain the same caller/child authority boundary rather than expose an unrestricted thread-control
shortcut.

- **Start in:** `crates/fleet-cli/src/commands/subagents.rs` and the daemon's agent gate controls,
  adding caller-authorized inspection and answer requests.
- **Done when:** the caller can list a delegated child's pending gates and answer each through the
  CLI, and another thread cannot answer them; human and JSON paths have end-to-end tests.

## 14. A read-only Jobs projection of delegations

**Impact: low.** Delegations appear in their caller's transcript, but the Jobs screen does not give
operators one place to scan all live and recent delegated work.

Delegations are not jobs and must not acquire job controls or a second lifecycle. The useful later
addition is a read-only projection of the durable delegation records.

- **Start in:** `crates/fleet-app/src/screens/jobs.rs` and
  `crates/fleet-app/src/screens/jobs/presentation.rs`, fed by delegation list/change data.
- **Done when:** the Jobs screen can filter and inspect delegation status, provider, caller, child,
  age and delivery without mutating a delegation or duplicating its state.

---

## Hygiene

- **`crates/fleet-app/src/keymap.rs` is 1251 lines**, over the ~900 guideline. It was already
  1138 before this stage, so the rebuild added to a pre-existing oversize file rather than creating
  one. Splitting a keymap table is its own change.
- **`crates/fleet-daemon/src/services/agents/store/import.rs` is 905 lines**, marginally over.
- **Four more files crossed ~900 lines in round 2**: `crates/fleet-app/src/screens/workspace/agent.rs`
  (946), `crates/fleet-app/src/screens/agent_thread/actions.rs` (950), `crates/fleet-daemon/src/machines/link.rs`
  (1003) and `crates/fleet-daemon/src/services/agents/store/import.rs` (909). `fleet-client/src/connection.rs`
  and `fleet-daemon/src/server/connection.rs` were already far over. Each split is its own change.
- **`Services::build` is infallible across twenty call sites**, which is why an unopenable agent
  database is fatal for the agent service rather than the daemon
  (`docs/decisions/0013-sqlite-agent-transcripts.md`). Making the composition fallible, so the
  process exits the way an unreadable `state.json` makes it exit, belongs with `Services::build`,
  not with the agent store.
- **The agent GUI smoke pass is fixture-driven.** The checked-in `scenarios/agents/` corpus
  covers popup attachment, focus, streaming, decisions, unread state, prefix routing, joined
  Codex diffs and transcript wheel input without a real provider binary or network.
- **The Codex approval round-trip has no live fixture.** The shapes in
  `docs/research/harness-codex-app-server.md` §3 come from the generated schema for
  `codex-cli 0.147.0` and are authoritative, but the live round-trip was never captured because
  the probe turn failed upstream on model capacity. Capture one before trusting the adapter's
  approval path in anger.
