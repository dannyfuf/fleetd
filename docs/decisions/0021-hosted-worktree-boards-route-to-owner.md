# 0021 — A hosted worktree's board routes to the daemon that owns the worktree

**Adopted** for the daemon's `Router` and `Boards` services, and for every client that opens a
worktree board. A worktree board is a document of the daemon that owns the worktree; the local
daemon forwards every board and card request addressed to it. Context boards stay local.
**Supersedes the addendum of [ADR 0019](0019-worktree-scoped-boards.md).**

## Context

A laptop connected to a dev-box host opened the board of a worktree the dev-box owns and saw it
empty, while the dev-box's own document for that worktree held eleven cards. Every board request
classified as `Target::Local`, so the laptop created and served
`boards/wt-<owner>-<repo>-<slug>.json` of its own.

ADR 0019's addendum chose "the requester's daemon owns the document" and made the boards service
resolve the worktree through the mirror so the local `ensure` would succeed. That is the wrong
daemon. A worktree board is the planning surface of work that happens on the machine that holds the
worktree: the board-workflows feature turns a card move into a delegation run on the daemon that
owns the worktree, and the agents, terminals and jobs it starts live there. A document on the
requester's machine cannot drive them, and two clients on two machines see two different boards for
one worktree. The addendum's reason for not routing — that board and card requests carry no worktree
id, and a snapshot fragment carries no cards — is an argument about *lookup*, and lookup is what id
ownership tables already solve for sessions, terminals, jobs and agent threads.

## Decision

- **Route by ownership.** `EnsureWorktreeBoard`/`CreateWorktreeBoard` resolve the worktree's owner.
  `GetBoard`, `UpdateBoard`, `DeleteBoard`, `CreateCard`, `SyncBoard` and `DescribeBoardBackend`
  resolve board ownership. `UpdateCard`, `MoveCard`, `DeleteCard`, `AddCardComment`,
  `ResolveCardConflict` and `CreateWorktreeFromCard` resolve the card's board and then its owner.
  `EnsureBoard`, `CreateBoard` and `ListBoardBackends` stay local: a context board is a document of
  the daemon whose contexts define it.
- **Two ownership tables, no new id space.** `RemoteIds` gains `boards: BoardId -> HostId` and
  `cards: CardId -> BoardId`. Board ownership is registered from a host's `Snapshot.boards` — only
  summaries carrying a `worktree_id`, because a host's context-board ids collide with this daemon's
  own by construction — and from every forwarded `Board`, `Boards` and `Card` answer. `Mirror`
  answers `host_of_board` from the fragments while the tables are cold or a link is down.
  Registration only ever adds: a stale snapshot applied after a forwarded answer must not forget a
  board the owner just created.
  A Down transition clears a host's board ownership and Ready rebuilds it; card-to-board is a
  stable fact and is not cleared.
- **`ListBoards` fans out.** `context_id: None` goes to every host, the local list always runs, only
  worktree-scoped hosted summaries are kept, a hosted summary replaces a local one with the same id,
  a requested `context_id` filters hosted summaries by the host's own context id without rewriting
  it, and an unreachable host contributes an empty list rather than hiding the local boards.
- **Events are filtered, not translated.** A host's `BoardChanged` is republished only for a board
  this router attributes to that host; its context-board events are dropped.
- **The capability is checked on both ends.** The typed client still checks `board.worktree` before
  enqueueing, and the local daemon checks the owning host's advertised capability before forwarding,
  refusing with the same sentence prefixed `host <id>: ` — "this daemon does not support worktree
  boards; run `fleet daemon restart`".
- **A stale local document is retired, never merged.** Before each ensure/create it routes to a host
  for a worktree, the local daemon moves its own document for that worktree to its trash
  (`retire_hosted_worktree_board`) and logs one line: `info` naming the trashed path when it held no
  cards, `warn` naming the path and the card count when it did. Failure is warned and the request is
  forwarded anyway.
- **The mirror seam stays, for card links only.** `RemoteWorktrees` keeps a card on a context board
  able to link and create a worktree on another host (`CreateWorktreeFromCard { host }`). Board
  *scope* is local-only: ensure/create for a worktree only the mirror names is
  `not found: worktree <id>`, and a document scoped to one is skipped by `list`/`summaries`.
- **No wire change.** No new request, response, event or capability; `PROTOCOL_VERSION` stays 8 and
  no golden changes.

## Alternatives rejected

- **Keep ADR 0019's addendum and move cards by hand.** It leaves one worktree with a different board
  per machine, and every board-workflow delegation on the wrong daemon. Copying a `boards/*.json`
  between machines to recover is a manual step the router can make unnecessary, and merging two
  documents for one worktree would have to invent a conflict policy for cards that never synced.
- **A card mirror, or a new cross-host id space.** Board and card ids are already globally unique —
  a `wt-…` id names an owner and a repository, a `CardId` is a UUID v4 — so a bijective remap like
  the terminal and job counters buys nothing and adds a translation that can drift. Mirroring cards
  locally would make the local daemon a second writer of the owner's document.
- **Routing only the two ensure/create requests.** The board a client then holds is the host's, and
  every later `GetBoard`/`MoveCard` would fall back to a local document that does not exist.

## Consequences

Board ids and card ids pass through untranslated and stay globally unique; anything that would make
either recur per daemon breaks routing. No client computes a `wt-…` id: the only `BoardId` the app
or CLI holds is one a daemon answer carried, and `apply_board_view` admits a hosted view by the
`worktree_id` the document carries. A host's context boards are never addressable from another
daemon — they are excluded from ownership registration, from the `ListBoards` merge and from event
republication — so a future feature that wants cross-machine context boards needs its own decision,
not a widening of this one. A card request made while its link is down resolves again as soon as the
board has an owner, from the mirror fragment in the meantime. Two daemons must agree on
`board.worktree`: an old host is named in the refusal rather than the daemon the client is talking
to.

Provenance: `docs/REMOTE-MACHINES.md` §6, `docs/BOARD.md` §0/§4, ADR 0011's ownership routing, and
the addendum this record supersedes.
