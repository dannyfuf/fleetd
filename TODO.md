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
