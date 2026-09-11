# TODO — native agents

Everything the native-agent rebuild owes, in one place, ordered by what a user notices first.

`docs/NATIVE-AGENTS.md` §13 is authoritative for *status*; this file is the working list and says
what each item costs. Each entry names the file to start in, why it was left, and how to know it
is done. Nothing here is a known-broken surface: every one of these either says so where the user
meets it, or degrades to a narrower behaviour rather than a wrong one.

---

## 1. A Codex file-change approval shows paths, not the diff

**Impact: high.** This is the one item a user will call a bug.

`docs/NATIVE-AGENTS.md` §6.2 promises the approval card shows the diff — *"For an edit approval
Fleet shows what t3code cannot: **the diff**, joined by `itemId`"*. It does not, on Codex.

Codex's `item/fileChange/requestApproval` deliberately carries **no diff and no paths**; the
changes live on the `fileChange` item the request names by `itemId` (see
`docs/research/harness-codex-app-server.md` §3.2). So the card has to join the approval onto that
item. `GateKind::Permission` has no `item` field to carry the id, so the join has nothing to key
on and the card falls back to the path list.

- **Start in:** `crates/fleet-core/src/agents/gates.rs` — add `item: Option<ItemId>` to
  `GateKind::Permission`. Then the Codex adapter's approval mapping
  (`crates/fleet-daemon/src/agents/codex/approvals.rs`) populates it, and the dock
  (`crates/fleet-ui-kit/src/components/agent/`) renders `DiffView` when it resolves.
- **Cost:** small. One additive field, one populate, one render branch.
- **Done when:** a Codex edit approval renders its diff at bounded height, and one that arrives
  before its item shows a loading state rather than an empty card.

## 2. `^s e` draws nothing on Codex

**Impact: high.** A control the docs describe is absent.

§7 says the reasoning-effort options are **discovered per model** from
`model/list.supportedReasoningEfforts`, with the harness's own `description` strings, and that
Fleet must never hardcode a Codex ladder. Neither `model/list` nor `skills/list` is called, so the
vocabularies are empty and the menu is empty.

The empty menu is the *correct* degradation — drawing a guessed ladder would be worse, per
`DESIGN-SYSTEM.md` §7 — but it means the control does not work on Codex.

- **Start in:** `crates/fleet-daemon/src/agents/codex/session.rs`. Both are cursor-paginated;
  loop on `nextCursor`. Re-run `skills/list` on the `skills/changed` notification.
- **Cost:** small-to-medium. Two RPCs already in the generated method tables, plus plumbing the
  results into the thread summary.
- **Done when:** `^s e` on a Codex tab lists that model's legal efforts with their descriptions
  and defaults to `defaultReasoningEffort`; `$` lists real skills.

## 3. The amber dot comes back after an app restart

**Impact: medium.** Cosmetic, but it erodes trust in the one mark that means "this needs you".

`AgentMarkSeen` validates its cursor and records nothing, so the read cursor lives only in app
memory. Restart the app and every thread you had already read is marked unread again.

The storage is ready and waiting — `crates/fleet-daemon/src/services/agents/store/schema.rs`
declares:

```sql
CREATE TABLE seen (
  client_id TEXT    NOT NULL,   -- stable per install, sent in Hello
  thread_id TEXT    NOT NULL,
  seq       INTEGER NOT NULL,
  at        INTEGER NOT NULL,
  PRIMARY KEY (client_id, thread_id)
) WITHOUT ROWID;
```

That comment is aspirational: **`client_id` is not sent in Hello.** `HelloClient`
(`crates/fleet-proto/src/request.rs:45`) carries `kind`, `host_id` — which is only for a
forwarding daemon, not an app — and `capabilities`. There is no per-install client identity
anywhere on the wire, so the primary key has a column nothing can fill.

And it genuinely needs that column. Several clients can watch one thread: two windows, a laptop
and a remote machine, the `fleet agent` CLI. Keyed on `thread_id` alone, opening a thread in one
window would silently clear the dot everywhere else — worse than it resetting, because then the
dot is lying rather than merely stale.

- **Cost:** two additive, capability-gated fields — `HelloClient.client_id` and a `seen_seq` on
  the window response so a reconnecting client gets *its own* cursor back — plus the store write.
  No `PROTOCOL_VERSION` bump; the daemon matches the version literally and a bump would lock out
  older remote daemons.
- **Left because:** designing a client-identity scheme is a real protocol decision with
  multi-window and multi-machine consequences, and there was no evidence to base it on. Guessing
  one into the wire is far more expensive to undo than a stale dot.
- **Done when:** a thread read before an app restart is still read after it, and reading it in one
  window does not clear it in another.

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

`item_attachments` has no writer. §4.1 already grants the attachments directory via `--add-dir`
and §7.2 documents Claude's block-order rule (images before the final text block, because the CLI
only reads a streamed user message as a slash-command invocation when the last block is text), so
the design is settled — only the implementation is missing.

- **Start in:** `crates/fleet-ui-kit/src/components/agent/` for the pending-chip model, then the
  store writer, then both adapters' `UserInput` mapping. Codex takes structured `image` /
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

---

## Hygiene

- **`crates/fleet-app/src/keymap.rs` is 1138 lines**, over the ~900 guideline. It was already
  1095 before this work, so the rebuild added to a pre-existing oversize file rather than creating
  one. Splitting a keymap table is its own change.
- **`crates/fleet-daemon/src/services/agents/store/import.rs` is 905 lines**, marginally over.
- **`Services::build` is infallible across twenty call sites**, which is why an unopenable agent
  database is fatal for the agent service rather than the daemon
  (`docs/decisions/0013-sqlite-agent-transcripts.md`). Making the composition fallible, so the
  process exits the way an unreadable `state.json` makes it exit, belongs with `Services::build`,
  not with the agent store.
- **No GUI smoke pass for the agent tab.** ADR 0007's driver script has no agent-tab procedure.
  2 548 green tests say the contracts hold, not that the tab feels right.
- **The Codex approval round-trip has no live fixture.** The shapes in
  `docs/research/harness-codex-app-server.md` §3 come from the generated schema for
  `codex-cli 0.147.0` and are authoritative, but the live round-trip was never captured because
  the probe turn failed upstream on model capacity. Capture one before trusting the adapter's
  approval path in anger.
