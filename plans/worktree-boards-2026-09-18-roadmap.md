# Worktree-scoped boards — Roadmap
> Phase plans:
>  - Phase 1: ./worktree-boards-2026-09-18-phase-1-plan.md · ./worktree-boards-2026-09-18-phase-1-tracker.md
>  - Phase 2: ./worktree-boards-2026-09-18-phase-2-plan.md · ./worktree-boards-2026-09-18-phase-2-tracker.md
>  - Phase 3: ./worktree-boards-2026-09-18-phase-3-plan.md · ./worktree-boards-2026-09-18-phase-3-tracker.md

## Summary

Fleet has one kanban board per **context** (`docs/BOARD.md`). This initiative lets a user
optionally attach a second, finer-grained board to a **worktree**, so the work done inside one
worktree session can be planned card by card without polluting the context board. Nothing is
mandatory: a worktree with no board behaves exactly as today. The existing `fleet board …` CLI
gains a worktree selector so a coding agent running inside a worktree terminal can create,
read, edit, move, comment on and delete cards on that worktree's board. The app gains an
opt-in board tab inside the worktree's Workspace. No new board features (no new card fields,
no new backends, no new dialogs) are added; the only new capability is *where* a board can live.

A third phase fixes something the board work exposed rather than caused: every text field in
the app is bare (no selection, no copy or paste outside the agent composer, no word- or
line-wise deletion in most dialogs, no undo, no IME composition in dialogs), and the ui-kit has
three separate editing models that every new surface re-wires by hand. Phase 3 builds one
editing engine and one live input component and migrates every text surface onto it, so the
board dialogs, and everything after them, get a complete text field for free.

## Why this is phased

The work crosses every layer of the workspace (core model, daemon service, wire protocol,
client, CLI, GPUI app, harness, docs) and has a natural shippable seam in the middle. After
phase 1 the daemon, protocol and CLI understand worktree boards and an agent can drive one
end to end, while the app is untouched and its context board keeps working. Phase 2 adds the
GUI surface on top of a protocol that already exists and is already tested, which is the order
the repo's `rust-ipc-protocol` skill asks for (protocol and daemon first, capability-gated,
then consumers). Doing both in one plan would put the riskiest GPUI work (a second pane kind
in the Workspace, a scope-aware `BoardState`) on the same critical path as the daemon cascade
and the wire goldens.

## Phase list

### Phase 1 — Model, daemon, protocol, client and CLI
- **Goal:** A board can be scoped to a worktree; the daemon creates, lists, serves and
  cascades it; the CLI can select it explicitly or from the current worktree terminal.
- **Shippable state at end of phase:** An agent in a worktree terminal runs
  `fleet board --worktree card new "…"` and every other existing `fleet board` command
  against that worktree's board; the Hub board in the app is unchanged and never shows a
  worktree board's cards; deleting the worktree deletes its board.
- **Plan:** ./worktree-boards-2026-09-18-phase-1-plan.md
- **Tracker:** ./worktree-boards-2026-09-18-phase-1-tracker.md

### Phase 3 — One text input for the whole app
- **Goal:** A single editing engine and a single live `TextInput` component (single-line and
  multi-line modes) with selection, clipboard, word and line deletion, undo and IME, used by
  every text surface in the app and the lazygit overlay.
- **Shippable state at end of phase:** The three old input families are deleted, the keymap
  has one `text_input::*` family, every dialog and filter edits through the component, and the
  design system documents one rule for inputs.
- **Plan:** ./worktree-boards-2026-09-18-phase-3-plan.md
- **Tracker:** ./worktree-boards-2026-09-18-phase-3-tracker.md

### Phase 2 — App surface: the `fleet://board` Workspace tab
- **Goal:** Inside a worktree session, `ctrl-s b` opens (or selects) a Fleet-drawn board
  tab showing that worktree's board with the same keys, dialogs and palette rows as the Hub
  board.
- **Shippable state at end of phase:** The tab exists as a reserved `fleet://board` native
  command, the board screen is scope-aware, KEYMAP/UX-SPEC/APP-CONTRACTS describe it, and a
  harness scenario drives it.
- **Plan:** ./worktree-boards-2026-09-18-phase-2-plan.md
- **Tracker:** ./worktree-boards-2026-09-18-phase-2-tracker.md

## Seams between phases

- **Wire contract.** Phase 1 adds `RequestBody::EnsureWorktreeBoard { worktree_id }` and
  `RequestBody::CreateWorktreeBoard { worktree_id, name, prefix, backend }`, adds the optional
  `worktreeId` field to `Board` and `BoardSummary`, and publishes the capability string
  `board.worktree` in `HelloResponse.capabilities`. `PROTOCOL_VERSION` stays at 8. Phase 2
  consumes exactly these and nothing else; if phase 2 discovers it needs another wire field,
  that is a new task in phase 2 that also re-runs phase 1's golden tests.
- **Capability gating.** Every phase 2 affordance (`ctrl-s b`, the palette row) checks
  `supports_capability("board.worktree")` on the live connection and degrades to a toast
  ("this daemon does not support worktree boards") when absent. Phase 1's CLI does the same
  check and prints the `fleet daemon restart` hint.
- **Snapshot.** `Snapshot.boards` already carries `BoardSummary`; phase 1 makes each summary
  say which worktree, if any, it belongs to. Phase 2 reads that to know whether a session's
  worktree already has a board without issuing a request.
- **Board id lookup.** Phase 1 owns the rule "a context's board is the board with that
  `contextId` and **no** `worktreeId`"; phase 2 must never re-derive it client-side. The app
  always goes through `EnsureBoard(context)` or `EnsureWorktreeBoard(worktree)`.
- **Docs.** Phase 1 updates `docs/BOARD.md` §0, §2, §4, §5, §6, `README.md` and adds ADR
  0018. Phase 2 updates `docs/BOARD.md` §8, `docs/UX-SPEC.md`, `docs/KEYMAP.md`,
  `docs/APP-CONTRACTS.md`, `docs/ARCHITECTURE.md` (native tabs) and `docs/SWARM-INVENTORY.md`.
  Each doc change rides in the commit of the code it describes.

- **Phase 3 → Phase 2.** Phase 2 reuses the board dialogs as they are. If phase 3 lands first
  (recommended), phase 2 inherits the migrated dialogs and touches no input code. If phase 2
  lands first, phase 3's P3-T05 migrates the board dialogs once, in place; the board pane adds
  no input of its own either way. Phase 3 shares no files with phase 1.
- **Key contexts.** Phase 3 introduces the rule that a surface binding bare letters publishes
  them under a context word absent while an input is focused. Phase 2's `Workspace > Native >
  Board` context binds bare letters and its board filter owns an input, so phase 2 must follow
  that rule (the board filter already swaps the chain today).

## Cross-phase risks

- **Context-board lookup regression.** `Boards::context_board` today scans every document
  for a matching `contextId`. If phase 1 forgets to exclude worktree boards, the Hub could
  show a worktree board as the context board. Phase 1 has a dedicated regression test for
  this and phase 2 asserts it again in its harness scenario.
- **Stale board adoption.** Worktree ids (`owner/name#slug`) recur: delete and recreate a
  worktree with the same slug and the id is identical. Without the delete cascade, the new
  worktree would adopt the old cards. Phase 1 wires the cascade into the one place every
  deletion passes through and tests the prune path as well as the direct delete.
- **Single `BoardState`.** Phase 2 keeps one `BoardState` and gives it a scope, because the
  Hub and the Workspace are never visible at the same time. If a later feature needs both
  boards live at once, that is the assumption to revisit.
- **Sleep and wake.** A user-opened `fleet://board` tab is not in the configured `windows[]`,
  so sleeping the session closes it and waking does not restore it. Phase 2 accepts that
  (`ctrl-s b` brings it back in one keystroke) and documents the config path for users who
  want it permanent. Not a bug; a stated behaviour.

## Suggested order

1. Phase 1 (P1-T01 → P1-T08) and Phase 3 (P3-T01 → P3-T08) in parallel: they share no files.
   Run `make restart` after P1-T05 so the running daemon has the new requests before the CLI
   tasks are tested against it.
2. Phase 2 (P2-T01 → P2-T06) after both, so the board tab ships with the migrated dialogs.
   `make harness` is the gate for phases 2 and 3, not `make test`.
3. Open one PR per phase; the phase 1 and phase 3 PRs are independently mergeable. Phase
   numbers are stable identifiers, not the execution order.
