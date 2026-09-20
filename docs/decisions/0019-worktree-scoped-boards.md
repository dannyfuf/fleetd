# 0019 — Boards may be scoped to a worktree

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
  documents. Creation of either scope appends `-2` through `-99` when another board occupies its
  default id, so a worktree-derived id cannot permanently exclude a context with the same id.
- Worktree deletion owns the lifetime boundary. `Worktrees` exposes a late-bound
  `WorktreeCascade`, installed weakly after `Boards` is composed, and invokes it after the
  worktree has moved to trash. The board is bundled into that trash entry so restoring or expiring
  the worktree applies to its board too. The weak observer avoids an object-graph cycle; a cascade
  failure is warned and swallowed because the successful worktree move cannot be rolled back.
- `EnsureWorktreeBoard` and `CreateWorktreeBoard` are additive requests advertised by the
  `board.worktree` capability. `PROTOCOL_VERSION` remains 8. The typed client checks the capability
  before enqueueing either request, and the connection actor checks the newly negotiated
  connection again before dispatch, so every consumer inherits the guard across reconnects.
- `fleet board --worktree[=<owner/name#slug>]` selects the scoped board. A bare flag resolves the
  current `FLEET_SESSION`; an explicit id requires `=` so it cannot consume a subcommand name.
  Commands without it keep selecting the context board as before.

## Alternatives rejected

- **A separate worktree-board store or scope enum.** The optional field preserves the existing
  document, snapshot, backend, and card paths and lets older documents decode unchanged. A second
  store would duplicate all of them; an enum would make a compatible additive field needlessly
  reshape the persisted model.
- **Using the derived id as identity.** Derivation can collide and can change in a future build.
  The persisted scope field is authoritative, so lookup remains correct across either event.
- **Cascading from every delete caller.** Direct deletion, repository deletion, and pruning all
  reach `Worktrees::delete_one`. Registering one observer there avoids three call sites that can
  drift and does not introduce a compile-time `Worktrees` → `Boards` dependency cycle.
- **Bumping the wire protocol.** Older daemons cannot decode the new request variants, but clients
  can avoid sending them after one capability check. Rejecting every mixed-version daemon pair
  would impose a fleet-wide upgrade for no benefit to clients that do not use scoped boards.
- **Making every worktree create a board.** The feature is optional. Eager creation would add
  documents and board noise for worktrees whose planning remains on the context board.

## Consequences

Old board documents and snapshots continue to decode with no worktree scope, and context-board
lookup must explicitly reject scoped boards. Listings skip a scoped board after its worktree is
gone, while the deletion cascade removes the live document so a repeated worktree id cannot
inherit it. The recoverable copy stays inside the worktree trash entry and returns on restore.
Unreadable live documents and quarantined remains survive cascades when their persisted scope
cannot be verified; a shared derived filename is not ownership evidence. The CLI and phase 2 app
surface must capability-gate the new requests.

Phase 2 deliberately keeps a single scope-aware `BoardState`: the Hub context board and a
Workspace worktree board are never visible at the same time. If a later feature displays or keeps
both boards live concurrently, that single-state assumption must be revisited rather than sharing
one generation, selection, loading flag, or cached view between the two scopes.

Phase 2 shipped that surface and added three things to this record. **The tab**: `fleet://board`
is now a second reserved native command, opened on demand by `ctrl-s b` in a worktree Workspace
and drawn by the app's own board screen — the same one the Hub tab uses, lent by `Shell` to
whichever surface is drawing, since the two are never on screen together. **The scope**:
`BoardState.scope` is `Context(ContextId) | Worktree(WorktreeId)`, `None` resolving to the active
context; entering a scope invalidates the mirror through the existing generation counter, so a
reply from the scope just left is stranded exactly as a reply from the previous context is, and
the Hub can never inherit a worktree scope. **The sleep/wake statement**: because Fleet never
writes the tab into `windows[]`, sleeping a session closes it and waking does not restore it;
that is a product decision recorded in `docs/UX-SPEC.md` §3.6, reversible by a user's own
`windows[]` entry and by a future default change, not by code.

No phase-1 assumption changed. The roadmap's rule that the app only ever reaches a board through
`EnsureBoard` or `EnsureWorktreeBoard` holds in the code: `screens::board::ensure_current` picks
the request from the scope, `AppState::apply_board_view` admits a view only when the scope does
(a worktree board must carry that worktree; a context board that context and **no** worktree),
and the only `BoardId` `fleet-app` ever holds is the one a daemon answer carried — the derived
`wt-…` id is never computed client-side. The capability gate holds too: the app refuses to enter
a worktree scope on a daemon without `board.worktree`, with the CLI's own sentence as a toast,
and the typed client re-checks at dispatch.

The protocol already exposes `DeleteBoard`, but `fleet board delete` remains a separate follow-up;
today a worktree board is removed with its worktree or through another typed client.

## Addendum: a worktree board for a mirrored remote worktree

**Superseded by [ADR 0021](0021-hosted-worktree-boards-route-to-owner.md) on 2026-09-20:** a
worktree board now lives on the daemon that owns the worktree and every board and card request for
it is routed there. The rest of this addendum is kept as the record of what was tried first.

A Workspace session on a worktree another host owns opens its board with the same
`EnsureWorktreeBoard` every local worktree uses, and that board is **owned by the daemon whose
client asked for it**, stored in that daemon's board store and resolved against its contexts.
`classify.rs` keeps every board request `Target::Local`.

Routing board requests to the owning host was rejected. Only the two ensure/create requests name
a worktree at all; every other request in the family is addressed by `board_id` or `card_id`
alone, and a mirrored snapshot fragment carries no boards' cards, so nothing the router or the
app holds could say which host a card belongs to. Federating boards would mean a new
identifier space, a card mirror, and cross-host card ids; scoping a locally owned board to a
remote worktree needs none of that, and matches the cards that already link a worktree on another
host through `CreateWorktreeFromCard { host }`.

The consequence is that the worktree such a board is scoped to is not in `State.worktrees`. The
`Boards` service therefore takes a second late-bound seam beside `WorktreeCascade` —
`RemoteWorktrees`, implemented for `Mirror` and installed in composition once the mirror exists —
and every worktree-existence check in the service reads local state first and the mirror second.
A worktree named by neither is still `not found: worktree <id>`. The board's context comes from
the local repository with the same derived `owner/name` id when there is one, else the first
context whose `owners` collect the repository's owner, else the active context, else
`not found: context for remote worktree <id>`. Nothing is bundled into trash when such a worktree
is deleted on its owner: the board is hidden by the same summaries filter that hides a board whose
local worktree is gone, and returns if the owner recreates the worktree.
