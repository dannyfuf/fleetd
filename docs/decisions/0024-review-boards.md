# 0024 — Review boards: a second board per context whose cards run in their own worktrees

**Adopted** for `fleet_core::board`, the daemon's `Boards`, the board protocol family,
`fleet board`, and the app's Pull requests screen. A context may hold one **Reviews board** beside
its task board. Its cards carry a pull request, its columns review each pull request in that pull
request's own worktree, and the Pull requests screen's Review tab shows it. Delivered in four
phases; the model lands first, and nothing a user sees changes until the daemon serves it.

## Context

Review requests reach the user from GitHub (assigned as reviewer) and from sources GitHub does not
know about — a company MCP server that posts requests in a chat channel. The Review tab lists the
first kind with `gh pr list --search user-review-requested:@me` and nothing else: it is flat and
stateless, so it cannot say that a review is under way, that an agent has finished one, that it has
been published, or what the agent found.

The board already has everything the missing half needs except one thing. ADR 0022 made a column
able to run a card as a native subagent, with a report, an outcome, routing and a throttle. But a
board runs every card in *one* checkout, the board's worktree, and a context board has none, so it
refuses automation outright. Reviewing five pull requests needs five checkouts, one per pull
request, in whichever repository each belongs to.

## Decision

- **A second board per context, not a replacement for the first.** `Board.kind` is `Tasks` or
  `Reviews`; `EnsureReviewsBoard(context)` gets or creates the one Reviews board
  (`reviews-<context>`), and the context board lookup accepts only `Tasks` boards, so the Hub's
  Board tab never shows the Reviews board. The Review tab shows it: that tab already answers "what
  is waiting on me", already owns `p` / `g p`, the context-bar Review chip and
  open-or-create-worktree, and a board adds what the flat list cannot hold — progress, the report,
  history. The **Mine** tab is unchanged.
- **Runs execute where the card says.** `BoardSettings.run_location` is `BoardWorktree` (every
  board before this) or `CardWorktree`: each run executes in the card's own `worktree_id`, created
  from the card's pull request on its first run and adopted when it already exists. That setting —
  and only that setting — lifts "automation is available on worktree boards only" for a context
  board. The worktree is created **outside** the board gate, because a fetch can take minutes, and
  under the run's reservation, so a slow clone counts against `max_live_runs`.
- **A card's pull request is immutable and unique per board.** `Card.pull_request` is set at
  creation and has no `CardPatch` field: a card *is* the review of one pull request. Document
  validation refuses a second card for the same `owner/name#n`, so two racing upserts can never
  persist a duplicate. `UpsertPullRequestCard` answers `Created`, `Existing` or `Reopened`, which is
  what makes it safe for an agent to run for every request it sees (ADR 0025).
- **Reopen only on a newer request.** A completed or archived card reopens — `Review re-requested`
  — only when `requested_at` is later than its last completion. A dismissed card never reopens:
  dismissal is a decision, and a chat message that stays in its channel forever must not undo it.
- **Routing columns queue.** A card that *enters* a column with `advance_when_unblocked` and has
  nothing blocking it advances at once, or, when the target is out of run slots, waits in place
  with a `pending_run` naming the target (`queued`) and is moved when a slot frees. Before this a
  card nothing blocked, standing in a routing column, was released by nothing; a card created in
  *Pending review* would have waited forever. The workflow preset's Ready column inherits the
  rule: a card moved into Ready with nothing blocking it starts at once. A queued card does not
  raise attention — waiting in line is the throttle working.
- **Only one column posts.** The review column's instructions forbid posting, committing and
  pushing, and report a verdict, a summary and numbered findings. *Review published* is the one
  column whose action posts, as one `gh` review, after applying the user's own comments on the card
  (the brief's `## Notes from you`). A person moves the card there; nothing routes into it.
- **The brief names the pull request.** `render_card_template` adds `{pr_url}`, `{pr_repo}` and
  `{pr_number}`, left as written on a card without a pull request so a Tasks board may still print
  them; the brief gains `## Pull request` and `## Notes from you`.
- **Version 3 is stamped lazily**, exactly as version 2 was (ADR 0022): only a board that uses a
  review field — a `Reviews` kind, a `CardWorktree` location, a card with a pull request, a run
  that records its worktree — writes 3.
- **Two runs at once.** A Reviews board defaults to `max_live_runs = 2`: runs no longer share a
  checkout, but they still spend tokens.

## Alternatives rejected

- **Replacing the whole Pull requests screen with a board.** The Mine tab ("what am I waiting on")
  has no board equivalent, and the screen is also the create-worktree-from-PR entry point.
- **A separate "Reviews" context holding an ordinary board.** A context groups repositories; a
  review context would own none, so its pull-request worktrees could not be created in it, and every
  review would live one context switch away from the work it belongs to.
- **Reviewing in the board's one worktree, one pull request at a time.** Checking out each pull
  request in turn would serialise every review, and a checkout switched under a live run corrupts
  what it reads.
- **Letting a card's pull request be patched.** A card retargeted to another pull request would
  carry a report, notes and history about a different change.
- **Cloning an unknown repository automatically.** Cloning is heavy and should be a choice; v1
  refuses the run with a sentence the card shows, and automatic cloning is a follow-up.

## Consequences

A board that uses a review field cannot be read by a daemon built before this feature: the older
store refuses version 3 by name rather than dropping the fields. Because every new run records the
worktree it ran in, a workflow board that runs a card after the upgrade writes 3 as well. Boards
that never use a review field are unaffected, byte for byte.

A context board automates only in `CardWorktree` mode; one that runs in its own (absent) worktree
still refuses with the original sentence. A card-worktree run adds two refusals, recorded on the
card as a failed run: `{KEY} has no worktree to run in; link a pull request or create its worktree
first` and `{owner/name} is not a Fleet repository; clone it into this context first`. The host
check moves to the start path, card by card. The delivery report diffs each run's own worktree and
never prints the shared-checkout sentence for a card-worktree board.

The capability `board.reviews` gates both new requests; it is advertised only by the build that
serves the whole flow, and `PROTOCOL_VERSION` stays 8. The app falls back to the flat review list
on a daemon without it.
