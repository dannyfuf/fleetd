# 0018 — Boards may be scoped to a worktree

**Adopted** for `fleet-core::board`, the daemon's `Boards` and `Worktrees` services, the board
protocol family, `fleet-client`, and `fleet board`. A context keeps its existing unscoped board
and each published worktree may additionally have one board of its own, created only on demand.

## Context

The context board is useful for project-wide work, but cards for one active worktree can obscure
that larger plan. Agents running inside Fleet terminals also know their worktree through
`FLEET_SESSION`; requiring them to discover and pass an unrelated board id would discard useful
scope Fleet already has.

Boards are stored as independent JSON documents and their ids are slugs, while a `WorktreeId` is
`owner/name#slug`. Worktree ids can recur after deletion, so leaving a scoped board behind would
let a recreated worktree adopt stale cards. The daemon's exact-version handshake also means a
protocol bump would force local and remote daemons to upgrade together for an optional feature.

## Decision

- `Board` and `BoardSummary` carry an optional, additive `worktree_id`. A worktree board keeps its
  repository's `context_id` and its worktree repository as `default_repo_id`, so existing context,
  repository, backend, card, and sync rules continue to apply. There is at most one board for a
  worktree.
- The default id is `wt-<owner>-<repository>-<slug>`, slugified and capped at the board-id limit.
  It is only a fast path. Lookup verifies the persisted `worktree_id` and falls back to scanning
  documents; creation appends `-2`, `-3`, and so on when another board occupies the derived id.
- Worktree deletion owns the lifetime boundary. `Worktrees` exposes a late-bound
  `WorktreeCascade`, installed after `Boards` is composed, and invokes it after the worktree has
  moved to trash. A cascade failure is warned and swallowed because the successful worktree move
  cannot be rolled back.
- `EnsureWorktreeBoard` and `CreateWorktreeBoard` are additive requests advertised by the
  `board.worktree` capability. `PROTOCOL_VERSION` remains 8. Consumers check the capability before
  sending either request.
- `fleet board --worktree [<owner/name#slug>]` selects the scoped board. A bare flag resolves the
  current `FLEET_SESSION`; commands without it keep selecting the context board as before.

## Alternatives rejected

- **A separate worktree-board store or scope enum.** The optional field preserves the existing
  document, snapshot, backend, and card paths and lets older documents decode unchanged. A second
  store would duplicate all of them; an enum would make a compatible additive field needlessly
  reshape the persisted model.
- **Using the derived id as identity.** Derivation can collide and can change in a future build.
  The persisted scope field is authoritative, so lookup remains correct across either event.
- **Cascading from every delete caller.** Direct deletion, repository deletion, and pruning all
  reach `Worktrees::delete_one`. Registering one observer there avoids three call sites that can
  drift and does not introduce a `Worktrees` → `Boards` dependency cycle.
- **Bumping the wire protocol.** Older daemons cannot decode the new request variants, but clients
  can avoid sending them after one capability check. Rejecting every mixed-version daemon pair
  would impose a fleet-wide upgrade for no benefit to clients that do not use scoped boards.
- **Making every worktree create a board.** The feature is optional. Eager creation would add
  documents and board noise for worktrees whose planning remains on the context board.

## Consequences

Old board documents and snapshots continue to decode with no worktree scope, and context-board
lookup must explicitly reject scoped boards. Listings skip a scoped board after its worktree is
gone, while the deletion cascade removes the document so a repeated worktree id cannot inherit it.
The CLI and phase 2 app surface must capability-gate the new requests.

Phase 2 deliberately keeps a single scope-aware `BoardState`: the Hub context board and a
Workspace worktree board are never visible at the same time. If a later feature displays or keeps
both boards live concurrently, that single-state assumption must be revisited rather than sharing
one generation, selection, loading flag, or cached view between the two scopes.

The protocol already exposes `DeleteBoard`, but `fleet board delete` remains a separate follow-up;
today a worktree board is removed with its worktree or through another typed client.
